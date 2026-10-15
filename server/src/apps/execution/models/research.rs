//! Run-scoped Research evidence: provider attempts with their cost attribution,
//! Source records, Document snapshots and delivered Evidence fragments. Every
//! read filters by Run, so another Run's identifier is simply absent.
use crate::{Error, Result};
use aidash_application::ports::research::{Reservation, Reserved};
use aidash_domain::research::{
	Operation,
	citations::EvidenceRef,
	document::{Completeness, Line},
	evidence::{
		AttemptOutcome, AttemptState, DocumentSnapshot, EvidenceFragment, Purge, PurgeReason,
		SourceRecord,
	},
	revocation::{EvidenceRevocation, Removed},
};
use chrono::{DateTime, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Json, Model, QueryRow};
use reinhardt::model;
use reinhardt::query::{
	Alias, Expr, ExprTrait, Func, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

const DISPATCHED: &str = "dispatched";
/// Advisory lock namespace of research reservations.
const LOCK_NAMESPACE: i64 = 71_003_237;

/// A transaction-scoped advisory lock serializing research writes of `key`.
pub(crate) async fn lock(tx: &mut dyn TransactionExecutor, key: &str) -> Result<()> {
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![SimpleExpr::FunctionCall(
				"hashtextextended".into_iden(),
				vec![Expr::value(key).into(), Expr::value(LOCK_NAMESPACE).into()],
			)],
		))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

#[model(app_label = "execution", table_name = "research_attempts")]
#[derive(Serialize, Deserialize)]
pub struct ResearchAttempt {
	#[field(primary_key = true, field_type = "uuid")]
	pub id: Uuid,
	#[field]
	pub run_id: Uuid,
	#[field(field_type = "text")]
	pub invocation_key: String,
	#[field(field_type = "text")]
	pub operation: String,
	#[field(field_type = "text")]
	pub provider: String,
	#[field(field_type = "text")]
	pub destination: String,
	/// The Search service operator's account charged for this attempt.
	#[field(field_type = "text", null = true)]
	pub account_id: Option<String>,
	#[field(null = true)]
	pub price_micro_usd: Option<i64>,
	#[field(field_type = "text")]
	pub outcome: String,
	#[field(null = true)]
	pub result: Option<Json<Value>>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	#[field(null = true)]
	pub completed_at: Option<DateTime<Utc>>,
}

#[model(app_label = "execution", table_name = "research_sources")]
#[derive(Serialize, Deserialize)]
pub struct ResearchSource {
	#[field(primary_key = true, field_type = "uuid")]
	pub id: Uuid,
	#[field]
	pub run_id: Uuid,
	#[field(field_type = "text")]
	pub url: String,
	#[field(field_type = "text")]
	pub title: String,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

#[model(app_label = "execution", table_name = "research_snapshots")]
#[derive(Serialize, Deserialize)]
pub struct ResearchSnapshot {
	#[field(primary_key = true, field_type = "uuid")]
	pub id: Uuid,
	#[field]
	pub run_id: Uuid,
	#[field]
	pub source_id: Uuid,
	#[field(field_type = "text")]
	pub requested_url: String,
	#[field(field_type = "text")]
	pub url: String,
	#[field(field_type = "text", null = true)]
	pub title: Option<String>,
	#[field(field_type = "text")]
	pub media_type: String,
	#[field]
	pub fetched_at: DateTime<Utc>,
	#[field(field_type = "text")]
	pub raw_digest: String,
	#[field(field_type = "text")]
	pub text_digest: String,
	#[field(field_type = "text")]
	pub completeness: String,
	/// Empty once the contents are purged.
	#[field]
	pub lines: Json<Value>,
	/// Zero once the contents are purged.
	#[field]
	pub text_bytes: i64,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	/// When the contents were purged; null while they are retained.
	#[field(null = true)]
	pub purged_at: Option<DateTime<Utc>>,
	/// `retention` or `revocation`, set together with `purged_at`.
	#[field(field_type = "text", null = true)]
	pub purge_reason: Option<String>,
}

#[model(app_label = "execution", table_name = "research_fragments")]
#[derive(Serialize, Deserialize)]
pub struct ResearchFragment {
	#[field(primary_key = true, field_type = "uuid")]
	pub id: Uuid,
	#[field]
	pub run_id: Uuid,
	#[field]
	pub snapshot_id: Uuid,
	#[field(field_type = "text")]
	pub text: String,
	#[field]
	pub line_start: i32,
	#[field]
	pub line_end: i32,
	#[field]
	pub pages: Json<Value>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

/// The audit record of one Evidence Revocation (ADR 0046). It does not
/// reference its Run, so it outlives the Run, and holds no source content.
#[model(app_label = "execution", table_name = "research_revocations")]
#[derive(Serialize, Deserialize)]
pub struct ResearchRevocation {
	#[field(primary_key = true)]
	pub run_id: Uuid,
	#[field(field_type = "text", null = true)]
	pub tenant: Option<String>,
	/// The [`Decider`](aidash_domain::retention::Decider) who revoked.
	#[field]
	pub revoked_by: Json<Value>,
	#[field(field_type = "text")]
	pub reason: String,
	#[field]
	pub revoked_at: DateTime<Utc>,
	/// What was removed: counts and snapshot text digests only.
	#[field]
	pub removed: Json<Value>,
}

fn stored(value: usize) -> i32 {
	i32::try_from(value).unwrap_or(i32::MAX)
}

/// Rows of `table` of one Run whose text `columns` equal the given values.
async fn count(
	tx: &mut dyn TransactionExecutor,
	table: &str,
	run: Uuid,
	columns: &[(&'static str, &str)],
) -> Result<u32> {
	let mut query = Query::select();
	query
		.expr_as(
			Func::count(Expr::col(Alias::new("id")).into()),
			Alias::new("n"),
		)
		.from(Alias::new(table))
		.and_where(Expr::col("run_id").eq(Expr::value(run)));
	for (column, value) in columns {
		query.and_where(Expr::col(*column).eq(Expr::value(*value)));
	}
	let (sql, values) = query.build(PostgresQueryBuilder);
	let row = tx.fetch_one(&sql, convert_values(values)).await?;
	let n = QueryRow::from_backend_row(row).data["n"]
		.as_i64()
		.unwrap_or(0);
	Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

impl ResearchAttempt {
	pub(crate) async fn find(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		key: &str,
	) -> Result<Option<AttemptState>> {
		let record = Self::objects()
			.filter(Self::field_run_id().eq(run))
			.filter(Self::field_invocation_key().eq(key.to_owned()))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop();
		Ok(
			record.map(|record| match (record.outcome.as_str(), record.result) {
				(DISPATCHED, _) | (_, None) => AttemptState::Dispatched,
				(outcome, Some(Json(result))) => AttemptState::Settled {
					outcome: match outcome {
						"completed" => AttemptOutcome::Completed,
						"failed" => AttemptOutcome::Failed,
						_ => AttemptOutcome::Unknown,
					},
					result,
				},
			}),
		)
	}

	pub(crate) async fn count(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		operation: Operation,
	) -> Result<u32> {
		count(
			tx,
			Self::table_name(),
			run,
			&[("operation", operation.as_str())],
		)
		.await
	}

	/// Attempts of `operation` still dispatched: their results are not yet
	/// recorded, so each may still need a Document snapshot.
	pub(crate) async fn in_flight(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		operation: Operation,
	) -> Result<u32> {
		count(
			tx,
			Self::table_name(),
			run,
			&[("operation", operation.as_str()), ("outcome", DISPATCHED)],
		)
		.await
	}

	/// Charged attempts of one account since `since`, including attempts whose
	/// outcome is not yet known.
	pub(crate) async fn spend(
		tx: &mut dyn TransactionExecutor,
		account: &str,
		since: DateTime<Utc>,
	) -> Result<u64> {
		let (sql, values) = Query::select()
			.expr_as(
				Func::coalesce(vec![
					Func::sum(Expr::col(Alias::new("price_micro_usd")).into()),
					Expr::value(0_i64).into(),
				]),
				Alias::new("spent"),
			)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("account_id").eq(Expr::value(account)))
			.and_where(Expr::col("created_at").gte(Expr::value(since)))
			.and_where(Expr::col("outcome").ne(Expr::value(AttemptOutcome::Failed.as_str())))
			.build(PostgresQueryBuilder);
		let row = tx.fetch_one(&sql, convert_values(values)).await?;
		let data = QueryRow::from_backend_row(row).data;
		let spent = data["spent"]
			.as_i64()
			.or_else(|| data["spent"].as_str().and_then(|text| text.parse().ok()))
			.unwrap_or(0);
		Ok(u64::try_from(spent).unwrap_or(0))
	}

	/// Reserve under transaction-scoped locks of the Run and, when charged, the
	/// account, so that concurrent invocations observe each other's attempts.
	pub(crate) async fn reserve(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		reservation: &Reservation,
	) -> Result<Reserved> {
		lock(tx, &format!("research-run:{run}")).await?;
		if let Some(attribution) = &reservation.attribution {
			lock(tx, &format!("research-account:{}", attribution.account_id)).await?;
		}
		if Self::find(tx, run, &reservation.key).await?.is_some() {
			return Ok(Reserved::Duplicate);
		}
		let limits = &reservation.limits;
		let maximum = match reservation.operation {
			Operation::Search => limits.search_attempts,
			Operation::Fetch | Operation::Find => limits.fetch_attempts,
		};
		if Self::count(tx, run, reservation.operation).await? >= maximum {
			return Ok(Reserved::AttemptLimit);
		}
		// A search, or a page read by address, needs room for a Source record.
		if (reservation.operation == Operation::Search || reservation.source_url.is_some())
			&& ResearchSource::count(tx, run).await? >= limits.sources
		{
			return Ok(Reserved::SourceLimit);
		}
		if reservation.operation == Operation::Fetch {
			// Every dispatched read may still record a snapshot, so it holds a
			// snapshot slot until it settles; concurrent reads cannot all pass.
			let (snapshots, bytes) = ResearchSnapshot::usage(tx, run).await?;
			let reading = Self::in_flight(tx, run, Operation::Fetch).await?;
			if snapshots.saturating_add(reading) >= limits.snapshots
				|| bytes >= limits.snapshot_bytes
			{
				return Ok(Reserved::SnapshotLimit);
			}
		}
		let attribution = reservation.attribution.as_ref();
		if let (Some(attribution), Some(budget)) = (attribution, &reservation.budget) {
			let spent = Self::spend(tx, &attribution.account_id, budget.since).await?;
			if spent.saturating_add(attribution.price_micro_usd) > budget.limit_micro_usd {
				return Ok(Reserved::BudgetExhausted);
			}
		}
		// Account profiles bound prices to the recorded range; never saturate.
		let price = attribution
			.map(|attribution| i64::try_from(attribution.price_micro_usd))
			.transpose()
			.map_err(|_| {
				Error::Invalid("research request price exceeds the recorded range".into())
			})?;
		let record = Self::build()
			.id(Uuid::now_v7())
			.run_id(run)
			.invocation_key(reservation.key.clone())
			.operation(reservation.operation.as_str().to_owned())
			.provider(reservation.provider.clone())
			.destination(reservation.destination.clone())
			.account_id(attribution.map(|attribution| attribution.account_id.clone()))
			.price_micro_usd(price)
			.outcome(DISPATCHED.to_owned())
			.result(None)
			.completed_at(None)
			.finish();
		Self::objects()
			.insert_with_executor(tx, &record)
			.await
			.map_err(FrameworkError::from)?;
		// The Source record of a page read by address holds its slot from now
		// on, under the same Run lock, so nothing is sent without it.
		let source = match &reservation.source_url {
			Some(url) => Some(ResearchSource::record(tx, run, url, "").await?.id),
			None => None,
		};
		Ok(Reserved::Reserved { source })
	}

	/// Settle a dispatched attempt once; a settled attempt is never rewritten.
	pub(crate) async fn settle(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		key: &str,
		outcome: AttemptOutcome,
		result: &Value,
	) -> Result<bool> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("outcome"), Expr::value(outcome.as_str()))
			.value_expr(Alias::new("result"), Expr::value(result.clone()))
			.value_expr(Alias::new("completed_at"), Expr::current_timestamp())
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("invocation_key").eq(Expr::value(key)))
			.and_where(Expr::col("outcome").eq(Expr::value(DISPATCHED)))
			.build(PostgresQueryBuilder);
		Ok(tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected
			== 1)
	}
}

impl ResearchSource {
	pub(crate) async fn record(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		url: &str,
		title: &str,
	) -> Result<SourceRecord> {
		let record = Self::build()
			.id(Uuid::now_v7())
			.run_id(run)
			.url(url.to_owned())
			.title(title.to_owned())
			.finish();
		Self::objects()
			.insert_with_executor(tx, &record)
			.await
			.map_err(FrameworkError::from)?;
		Ok(SourceRecord {
			id: record.id,
			run_id: run,
			url: record.url,
			title: record.title,
		})
	}

	pub(crate) async fn get(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		id: Uuid,
	) -> Result<Option<SourceRecord>> {
		Ok(Self::objects()
			.filter(Self::field_run_id().eq(run))
			.filter(Self::field_id().eq(id))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.map(|record| SourceRecord {
				id: record.id,
				run_id: record.run_id,
				url: record.url,
				title: record.title,
			}))
	}

	pub(crate) async fn count(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<u32> {
		count(tx, Self::table_name(), run, &[]).await
	}
}

impl ResearchSnapshot {
	pub(crate) async fn record(
		tx: &mut dyn TransactionExecutor,
		snapshot: &DocumentSnapshot,
	) -> Result<()> {
		let bytes = snapshot
			.lines
			.iter()
			.map(|line| line.text.len())
			.sum::<usize>();
		let record = Self::build()
			.id(snapshot.id)
			.run_id(snapshot.run_id)
			.source_id(snapshot.source_id)
			.requested_url(snapshot.requested_url.clone())
			.url(snapshot.url.clone())
			.title(snapshot.title.clone())
			.media_type(snapshot.media_type.clone())
			.fetched_at(snapshot.fetched_at)
			.raw_digest(snapshot.raw_digest.clone())
			.text_digest(snapshot.text_digest.clone())
			.completeness(snapshot.completeness.as_str().to_owned())
			.lines(Json(serde_json::to_value(&snapshot.lines)?))
			.text_bytes(i64::try_from(bytes).unwrap_or(i64::MAX))
			.purged_at(None)
			.purge_reason(None)
			.finish();
		Self::objects()
			.insert_with_executor(tx, &record)
			.await
			.map_err(FrameworkError::from)?;
		Ok(())
	}

	pub(crate) async fn get(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		id: Uuid,
	) -> Result<Option<DocumentSnapshot>> {
		Self::objects()
			.filter(Self::field_run_id().eq(run))
			.filter(Self::field_id().eq(id))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.map(|record| {
				Ok(DocumentSnapshot {
					id: record.id,
					run_id: record.run_id,
					source_id: record.source_id,
					requested_url: record.requested_url,
					url: record.url,
					title: record.title,
					media_type: record.media_type,
					fetched_at: record.fetched_at,
					raw_digest: record.raw_digest,
					text_digest: record.text_digest,
					completeness: if record.completeness == "complete" {
						Completeness::Complete
					} else {
						Completeness::TextLimit
					},
					lines: serde_json::from_value::<Vec<Line>>(record.lines.0)?,
					purged: match (record.purged_at, record.purge_reason.as_deref()) {
						(Some(at), Some(reason)) => Some(Purge {
							at,
							reason: PurgeReason::parse(reason).ok_or_else(|| {
								Error::Invalid("unknown snapshot purge reason".into())
							})?,
						}),
						_ => None,
					},
				})
			})
			.transpose()
	}

	/// Count and text bytes of one Run's retained snapshots. A Purged snapshot
	/// counts toward neither, so the Run can read its Source again (ADR 0045).
	pub(crate) async fn usage(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<(u32, u64)> {
		let (sql, values) = Query::select()
			.expr_as(
				Func::count(Expr::col(Alias::new("id")).into()),
				Alias::new("n"),
			)
			.expr_as(
				Func::coalesce(vec![
					Func::sum(Expr::col(Alias::new("text_bytes")).into()),
					Expr::value(0_i64).into(),
				]),
				Alias::new("bytes"),
			)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("purged_at").is_null())
			.build(PostgresQueryBuilder);
		let row = tx.fetch_one(&sql, convert_values(values)).await?;
		let data = QueryRow::from_backend_row(row).data;
		let number = |value: &Value| {
			value
				.as_i64()
				.or_else(|| value.as_str().and_then(|text| text.parse().ok()))
				.unwrap_or(0)
		};
		Ok((
			u32::try_from(number(&data["n"])).unwrap_or(u32::MAX),
			u64::try_from(number(&data["bytes"])).unwrap_or(0),
		))
	}
}

impl ResearchFragment {
	pub(crate) async fn record(
		tx: &mut dyn TransactionExecutor,
		fragment: &EvidenceFragment,
	) -> Result<()> {
		let record = Self::build()
			.id(fragment.reference.0)
			.run_id(fragment.run_id)
			.snapshot_id(fragment.document_id)
			.text(fragment.text.clone())
			.line_start(stored(fragment.line_start))
			.line_end(stored(fragment.line_end))
			.pages(Json(serde_json::to_value(&fragment.pages)?))
			.finish();
		Self::objects()
			.insert_with_executor(tx, &record)
			.await
			.map_err(FrameworkError::from)?;
		Ok(())
	}

	/// The fragments among `references` that were delivered to `run`.
	pub(crate) async fn delivered(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		references: &[EvidenceRef],
	) -> Result<Vec<EvidenceFragment>> {
		if references.is_empty() {
			return Ok(vec![]);
		}
		let ids = references
			.iter()
			.map(|reference| reference.0)
			.collect::<Vec<_>>();
		let records = Self::objects()
			.filter(Self::field_run_id().eq(run))
			.filter(Self::field_id().is_in(ids))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?;
		let mut fragments = Vec::with_capacity(records.len());
		for record in records {
			let Some(snapshot) = ResearchSnapshot::get(tx, run, record.snapshot_id).await? else {
				continue;
			};
			fragments.push(EvidenceFragment {
				reference: EvidenceRef(record.id),
				run_id: record.run_id,
				document_id: snapshot.id,
				source_id: snapshot.source_id,
				text_digest: snapshot.text_digest,
				text: record.text,
				line_start: usize::try_from(record.line_start).unwrap_or(0),
				line_end: usize::try_from(record.line_end).unwrap_or(0),
				pages: serde_json::from_value(record.pages.0)?,
			});
		}
		Ok(fragments)
	}
}

impl ResearchSnapshot {
	/// Remove the contents of the snapshots `ids` still retained, keeping
	/// their identity, retrieval time and digests. Returns how many lost them.
	pub(crate) async fn purge(
		tx: &mut dyn TransactionExecutor,
		ids: &[Uuid],
		at: DateTime<Utc>,
	) -> Result<u64> {
		if ids.is_empty() {
			return Ok(0);
		}
		let (sql, values) = Self::removal(at, PurgeReason::Retention)
			.and_where(Expr::col("id").is_in(ids.iter().map(|id| Expr::value(*id))))
			.and_where(Expr::col("purged_at").is_null())
			.build(PostgresQueryBuilder);
		Ok(tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected)
	}

	/// Remove the contents and titles of all of one Run's snapshots for an
	/// Evidence Revocation, whatever an earlier purge left. Returns their
	/// count and distinct text digests.
	pub(crate) async fn revoke(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		at: DateTime<Utc>,
	) -> Result<(u32, Vec<String>)> {
		let digests = Self::objects()
			.filter(Self::field_run_id().eq(run))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.into_iter()
			.map(|record| record.text_digest)
			.collect::<std::collections::BTreeSet<_>>();
		let (sql, values) = Self::removal(at, PurgeReason::Revocation)
			.value(Alias::new("title"), Option::<String>::None)
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.build(PostgresQueryBuilder);
		let removed = tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected;
		Ok((
			u32::try_from(removed).unwrap_or(u32::MAX),
			digests.into_iter().collect(),
		))
	}

	fn removal(at: DateTime<Utc>, reason: PurgeReason) -> reinhardt::query::UpdateStatement {
		let mut update = Query::update();
		update
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("lines"), Expr::cust("'[]'::jsonb"))
			.value(Alias::new("text_bytes"), 0_i64)
			.value(Alias::new("purged_at"), at)
			.value(Alias::new("purge_reason"), reason.as_str());
		update
	}
}

impl ResearchFragment {
	/// Remove the delivered text of all of one Run's fragments.
	pub(crate) async fn revoke(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<u32> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value(Alias::new("text"), "")
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.build(PostgresQueryBuilder);
		let removed = tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected;
		Ok(u32::try_from(removed).unwrap_or(u32::MAX))
	}
}

impl ResearchSource {
	/// Remove the titles of all of one Run's Source records; their URLs, the
	/// identity of each Source, remain.
	pub(crate) async fn revoke(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<u32> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value(Alias::new("title"), "")
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.build(PostgresQueryBuilder);
		let removed = tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected;
		Ok(u32::try_from(removed).unwrap_or(u32::MAX))
	}
}

impl ResearchAttempt {
	/// Replace the recorded result of every settled attempt of one Run with
	/// the tombstone of its operation. Attempts stay, with their charges.
	pub(crate) async fn revoke(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<u32> {
		let mut removed = 0;
		for operation in [Operation::Search, Operation::Fetch, Operation::Find] {
			let (sql, values) = Query::update()
				.table(Alias::new(Self::table_name()))
				.value(Alias::new("result"), revoked_result(operation))
				.and_where(Expr::col("run_id").eq(Expr::value(run)))
				.and_where(Expr::col("operation").eq(Expr::value(operation.as_str())))
				.and_where(Expr::col("result").is_not_null())
				.build(PostgresQueryBuilder);
			removed += tx
				.execute(&sql, convert_values(values))
				.await?
				.rows_affected;
		}
		Ok(u32::try_from(removed).unwrap_or(u32::MAX))
	}

	/// The operation of one attempt, to settle it with its tombstone.
	pub(crate) async fn operation_of(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		key: &str,
	) -> Result<Option<Operation>> {
		Ok(Self::objects()
			.filter(Self::field_run_id().eq(run))
			.filter(Self::field_invocation_key().eq(key.to_owned()))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.and_then(|record| Operation::parse(&record.operation)))
	}
}

/// Replace the stored tool results of research invocations with tombstones,
/// recognized by the shared result envelope like the harness recognizes
/// them: those of `run`, or with none those of every revoked Run, which
/// catches a result stored just after its Run was revoked. Idempotent.
pub(crate) async fn revoke_invocations(
	tx: &mut dyn TransactionExecutor,
	run: Option<Uuid>,
) -> Result<u32> {
	let mut removed = 0;
	for operation in [Operation::Search, Operation::Fetch, Operation::Find] {
		let tombstone = revoked_result(operation);
		let mut update = Query::update();
		update
			.table(Alias::new("invocations"))
			.value(Alias::new("result"), tombstone.clone())
			.and_where(Expr::cust("(result->>'version') = '1'"))
			.and_where(Expr::cust("jsonb_typeof(result->'limits') = 'object'"))
			.and_where(Expr::cust_with_values(
				"(result->>'operation') = ?",
				[operation.as_str()],
			))
			.and_where(Expr::col("result").ne(Expr::value(tombstone)));
		match run {
			Some(run) => update.and_where(Expr::col("run_id").eq(Expr::value(run))),
			None => update.and_where(
				Expr::col("run_id").in_subquery(
					Query::select()
						.column(Alias::new("run_id"))
						.from(Alias::new(ResearchRevocation::table_name()))
						.to_owned(),
				),
			),
		};
		let (sql, values) = update.build(PostgresQueryBuilder);
		removed += tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected;
	}
	Ok(u32::try_from(removed).unwrap_or(u32::MAX))
}

impl ResearchRevocation {
	/// Whether the Run's Research evidence was revoked.
	pub(crate) async fn exists(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<bool> {
		Ok(Self::find(tx, run).await?.is_some())
	}

	pub(crate) async fn find(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
	) -> Result<Option<EvidenceRevocation>> {
		Self::objects()
			.filter(Self::field_run_id().eq(run))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.map(Self::contract)
			.transpose()
	}

	pub(crate) async fn record(
		tx: &mut dyn TransactionExecutor,
		revocation: &EvidenceRevocation,
	) -> Result<()> {
		let record = Self::build()
			.run_id(revocation.run_id)
			.tenant(revocation.tenant.clone())
			.revoked_by(Json(serde_json::to_value(&revocation.revoked_by)?))
			.reason(revocation.reason.clone())
			.revoked_at(revocation.revoked_at)
			.removed(Json(serde_json::to_value(&revocation.removed)?))
			.finish();
		Self::objects()
			.insert_with_executor(tx, &record)
			.await
			.map_err(FrameworkError::from)?;
		Ok(())
	}

	/// One Tenant's records, newest first.
	pub(crate) async fn list(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		offset: u64,
		limit: u32,
	) -> Result<Vec<EvidenceRevocation>> {
		Self::objects()
			.filter(Self::field_tenant().eq(tenant.to_owned()))
			.order_by(&["-revoked_at", "run_id"])
			.offset(usize::try_from(offset).unwrap_or(usize::MAX))
			.limit(limit as usize)
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.into_iter()
			.map(Self::contract)
			.collect()
	}

	fn contract(record: Self) -> Result<EvidenceRevocation> {
		Ok(EvidenceRevocation {
			run_id: record.run_id,
			tenant: record.tenant,
			revoked_by: serde_json::from_value(record.revoked_by.0)?,
			reason: record.reason,
			revoked_at: record.revoked_at,
			removed: serde_json::from_value::<Removed>(record.removed.0)?,
		})
	}
}

/// What a research result is replaced with once its Run's evidence is revoked.
pub(crate) fn revoked_result(operation: Operation) -> Value {
	aidash_domain::research::revocation::tombstone(operation)
}
