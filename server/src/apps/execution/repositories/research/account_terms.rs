//! Provider results under the Research account terms (ADR 0058): the sweep
//! that purges those without a basis, run in Deployment Scope on the hourly
//! retention loop, and the expired results of one Run.
use super::native;
use crate::apps::execution::models::research::{
	ExpiringAttempts, ResearchAccountTermination, ResearchAttempt, ResearchRevocation,
	ResearchSnapshot, ResearchSource,
};
use crate::store::Store;
use aidash_application::{
	Result,
	ports::research::{ProviderResultRetention, RetainedResult},
};
use aidash_domain::research::{
	Operation,
	account::{AccountTerms, Termination},
	evidence::PurgeReason,
	expiry::ExpiredResults,
};
use async_trait::async_trait;
use chrono::{DateTime, SubsecRound, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{Model, QueryRow, execution::convert_values};
use reinhardt::query::{
	Alias, Expr, ExprTrait, Func, OnConflict, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use std::collections::BTreeMap;
use uuid::Uuid;

pub(crate) struct ProviderResults {
	pub store: Store,
}

#[async_trait]
impl ProviderResultRetention for ProviderResults {
	async fn record_terminations(&self, terminations: &[Termination]) -> Result<()> {
		native(
			async {
				let mut tx = super::retention_transaction(&self.store).await?;
				for termination in terminations {
					super::lock(
						tx.as_mut(),
						&format!("research-account:{}", termination.account_id),
					)
					.await?;
					let (sql, values) = Query::insert()
						.into_table(Alias::new(ResearchAccountTermination::table_name()))
						.columns(
							[
								"account_id",
								"terminated_at",
								"deletion_after_termination_days",
								"recorded_at",
							]
							.map(Alias::new),
						)
						.values_panic([
							Expr::value(termination.account_id.clone()),
							Expr::value(termination.terminated_at),
							Expr::value(i32::from(termination.deletion_after_termination_days)),
							Expr::value(Utc::now()),
						])
						// A recorded end is never replaced.
						.on_conflict(OnConflict::column(Alias::new("account_id")).do_nothing().to_owned())
						.build(PostgresQueryBuilder);
					tx.execute(&sql, convert_values(values)).await?;
				}
				tx.commit().await?;
				Ok(())
			}
			.await,
		)
	}
	async fn terminations(&self) -> Result<Vec<Termination>> {
		native(
			async {
				let mut tx = super::retention_transaction(&self.store).await?;
				let records = ResearchAccountTermination::objects()
					.all()
					.all_with_executor(tx.as_mut())
					.await
					.map_err(FrameworkError::from)?;
				tx.commit().await?;
				Ok(records
					.into_iter()
					.map(|record| Termination {
						account_id: record.account_id,
						terminated_at: record.terminated_at,
						deletion_after_termination_days: u16::try_from(
							record.deletion_after_termination_days,
						)
						.unwrap_or(u16::MAX),
					})
					.collect())
			}
			.await,
		)
	}
	async fn candidates(
		&self,
		after: Option<Uuid>,
		retrieved_before: DateTime<Utc>,
		accounts: &[String],
		limit: u32,
	) -> Result<Vec<RetainedResult>> {
		let attempt = |column: &str| {
			(
				Alias::new(ResearchAttempt::table_name()),
				Alias::new(column),
			)
		};
		// Dispatch is the conservative durable age. A late recovery settlement
		// must never give already-recorded provider artifacts a new lifetime.
		let retrieved_at = || Expr::col(attempt("created_at"));
		let mut due = retrieved_at().lte(Expr::value(retrieved_before));
		if !accounts.is_empty() {
			due = due.or(Expr::col(attempt("retention_account_id"))
				.is_in(accounts.iter().map(|account| Expr::value(account.clone()))));
		}
		let mut query = Query::select();
		query
			.columns(["id", "retention_account_id", "retention_days"].map(attempt))
			.expr_as(retrieved_at(), Alias::new("retrieved_at"))
			.from(Alias::new(ResearchAttempt::table_name()))
			.and_where(Expr::col(attempt("retention_account_id")).is_not_null())
			.and_where(Expr::col(attempt("purged_at")).is_null())
			// A settled result, or provider content an unsettled attempt
			// already stored, which a crash may leave for good.
			.and_where(
				Expr::col(attempt("result"))
					.is_not_null()
					.or(Expr::exists(produced(ResearchSource::table_name())))
					.or(Expr::exists(produced(ResearchSnapshot::table_name()))),
			)
			// A revoked Run's results are tombstones already (ADR 0046).
			.and_where(
				Expr::col(attempt("run_id")).not_in_subquery(
					Query::select()
						.column(Alias::new("run_id"))
						.from(Alias::new(ResearchRevocation::table_name()))
						.to_owned(),
				),
			)
			.and_where(due)
			.order_by(attempt("id"), Order::Asc)
			.limit(u64::from(limit));
		if let Some(after) = after {
			query.and_where(Expr::col(attempt("id")).gt(Expr::value(after)));
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		native(
			async {
				let mut tx = super::retention_transaction(&self.store).await?;
				let rows = tx.fetch_all(&sql, convert_values(values)).await?;
				tx.commit().await?;
				rows.into_iter()
					.map(|row| {
						let data = QueryRow::from_backend_row(row).data;
						Ok(RetainedResult {
							attempt_id: serde_json::from_value(data["id"].clone())?,
							account_id: serde_json::from_value(
								data["retention_account_id"].clone(),
							)?,
							retrieved_at: serde_json::from_value(data["retrieved_at"].clone())?,
							retention_days: number(&data["retention_days"])
								.and_then(|days| u16::try_from(days).ok())
								.unwrap_or(1),
						})
					})
					.collect()
			}
			.await,
		)
	}
	async fn purge(&self, attempts: &[Uuid], at: DateTime<Utc>) -> Result<u64> {
		if attempts.is_empty() {
			return Ok(0);
		}
		// The watermark a projection records is read back from these rows.
		let at = at.trunc_subsecs(6);
		native(
			async {
				let mut tx = super::retention_transaction(&self.store).await?;
				let records = ResearchAttempt::objects()
					.filter(ResearchAttempt::field_id().is_in(attempts.to_vec()))
					.all_with_executor(tx.as_mut())
					.await
					.map_err(FrameworkError::from)?;
				let mut runs = BTreeMap::<Uuid, Vec<(String, Operation)>>::new();
				for record in records
					.into_iter()
					.filter(|record| record.purged_at.is_none())
				{
					if let Some(operation) = Operation::parse(&record.operation) {
						runs.entry(record.run_id)
							.or_default()
							.push((record.invocation_key, operation));
					}
				}
				let mut purged = 0;
				for (run, keys) in runs {
					purged += ExpiringAttempts { run, keys }
						.expire(tx.as_mut(), at)
						.await?;
				}
				tx.commit().await?;
				Ok(purged)
			}
			.await,
		)
	}
}

/// Rows of `table` that an attempt stored under its invocation key.
fn produced(table: &str) -> reinhardt::query::SelectStatement {
	let attempt = |column: &str| {
		(
			Alias::new(ResearchAttempt::table_name()),
			Alias::new(column),
		)
	};
	Query::select()
		.expr(Expr::value(1_i64))
		.from(Alias::new(table))
		.and_where(Expr::col((Alias::new(table), Alias::new("run_id"))).equals(attempt("run_id")))
		.and_where(
			Expr::col((Alias::new(table), Alias::new("attempt_key")))
				.equals(attempt("invocation_key")),
		)
		.to_owned()
}

fn number(value: &serde_json::Value) -> Option<i64> {
	value
		.as_i64()
		.or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

/// The Provider results of `run` purged under the Research account terms, by
/// the identities its research results name.
pub(crate) async fn expired(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
) -> crate::Result<ExpiredResults> {
	let purged = ResearchAttempt::objects()
		.filter(ResearchAttempt::field_run_id().eq(run))
		.filter(
			ResearchAttempt::field_purge_reason()
				.eq(Some(PurgeReason::AccountTerms.as_str().to_owned())),
		)
		.all_with_executor(tx)
		.await
		.map_err(FrameworkError::from)?;
	let Some(through) = purged.iter().filter_map(|attempt| attempt.purged_at).max() else {
		return Ok(ExpiredResults::default());
	};
	let keys = |operation: Operation| {
		purged
			.iter()
			.filter(|attempt| attempt.operation == operation.as_str())
			.map(|attempt| Some(attempt.invocation_key.clone()))
			.collect::<Vec<_>>()
	};
	let (searches, reads) = (keys(Operation::Search), keys(Operation::Fetch));
	let sources = if searches.is_empty() {
		Default::default()
	} else {
		ResearchSource::objects()
			.filter(ResearchSource::field_run_id().eq(run))
			.filter(ResearchSource::field_attempt_key().is_in(searches))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.into_iter()
			.map(|source| source.id)
			.collect()
	};
	let documents = if reads.is_empty() {
		Default::default()
	} else {
		ResearchSnapshot::objects()
			.filter(ResearchSnapshot::field_run_id().eq(run))
			.filter(ResearchSnapshot::field_attempt_key().is_in(reads))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.into_iter()
			.map(|snapshot| snapshot.id)
			.collect()
	};
	Ok(ExpiredResults {
		sources,
		documents,
		through: Some(through),
	})
}

/// Runs examined per round trip of the projection scrub.
const SCRUB_BATCH: u64 = 64;

/// Scrub the stored projections of Runs with Provider results purged after
/// the latest purge their projection records, such as one a worker saved
/// while the sweep purged. Returns how many were scrubbed.
pub(crate) async fn scrub_expired_projections(store: &Store) -> Result<u32> {
	let attempt = |column: &str| (Alias::new("a"), Alias::new(column));
	let mut scrubbed = 0;
	let mut skipped = Vec::new();
	let mut after: Option<Uuid> = None;
	loop {
		let mut query = Query::select();
		query
			.column((Alias::new("r"), Alias::new("id")))
			.from_as(Alias::new("runs"), Alias::new("r"))
			.and_where(
				Expr::col((Alias::new("r"), Alias::new("id"))).in_subquery(
					Query::select()
						.column(attempt("run_id"))
						.from_as(Alias::new(ResearchAttempt::table_name()), Alias::new("a"))
						.and_where(
							Expr::col(attempt("purge_reason"))
								.eq(PurgeReason::AccountTerms.as_str()),
						)
						.and_where(Expr::col(attempt("purged_at")).gt(Func::coalesce(vec![
								crate::apps::execution::models::research::json_text(
									Expr::col((Alias::new("r"), Alias::new("context"))),
									"research_expired_through",
								)
								.cast_as("timestamptz"),
								Expr::value("-infinity").cast_as("timestamptz"),
							])))
						.to_owned(),
				),
			)
			.order_by((Alias::new("r"), Alias::new("id")), Order::Asc)
			.limit(SCRUB_BATCH);
		if let Some(after) = after {
			query.and_where(Expr::col((Alias::new("r"), Alias::new("id"))).gt(Expr::value(after)));
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		let runs = native(
			async {
				let mut tx = super::retention_transaction(store).await?;
				let rows = tx.fetch_all(&sql, convert_values(values)).await?;
				tx.commit().await?;
				Ok(rows)
			}
			.await,
		)?
		.into_iter()
		.filter_map(|row| {
			QueryRow::from_backend_row(row).data["id"]
				.as_str()
				.and_then(|id| id.parse::<Uuid>().ok())
		})
		.collect::<Vec<_>>();
		let Some(last) = runs.last().copied() else {
			break;
		};
		after = Some(last);
		for run in &runs {
			let mut tx = native(super::retention_transaction(store).await)?;
			let done = super::revocation::scrub_projection(
				tx.as_mut(),
				*run,
				super::revocation::Scrub::Expiry,
			)
			.await?;
			native(tx.commit().await.map_err(crate::Error::from))?;
			if done {
				scrubbed += 1;
			} else {
				skipped.push(*run);
			}
		}
		if (runs.len() as u64) < SCRUB_BATCH {
			break;
		}
	}
	for run in skipped {
		scrubbed += u32::from(
			super::revocation::scrub_retrying(store, run, super::revocation::Scrub::Expiry).await?,
		);
	}
	Ok(scrubbed)
}

/// One account-terms pass under `current`, the account terms the Deployment
/// declares now: purge every Provider result without a basis, then scrub the
/// stored projections that still carry one. Returns how many attempts were
/// purged.
pub(crate) async fn sweep(store: &Store, current: &[AccountTerms]) -> Result<u64> {
	let purged = aidash_application::research::account_terms::sweep(
		&ProviderResults {
			store: store.clone(),
		},
		current,
		Utc::now(),
		aidash_application::research::account_terms::BATCH,
	)
	.await?;
	scrub_expired_projections(store).await?;
	Ok(purged)
}
