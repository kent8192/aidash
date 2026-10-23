//! Run-scoped Research evidence: provider attempts with their cost attribution,
//! Source records, Document snapshots and delivered Evidence fragments. Every
//! read filters by Run, so another Run's identifier is simply absent.
use crate::{Error, Result};
use aidash_application::ports::research::{Reservation, Reserved};
use aidash_domain::research::{
	Failure, Operation,
	citations::EvidenceRef,
	document::{Completeness, Line},
	evidence::{
		AttemptOutcome, AttemptState, DocumentSnapshot, EvidenceFragment, FragmentImage, Original,
		Purge, PurgeReason, SourceRecord,
	},
	revocation::{EvidenceRevocation, Removed},
};
use chrono::{DateTime, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Json, Model, QueryRow};
use reinhardt::model;
use reinhardt::query::{
	Alias, Expr, ExprTrait, Func, IntoIden, LockType, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

mod journal_results;
pub(crate) use journal_results::ResearchJournalResult;

const DISPATCHED: &str = "dispatched";
/// Advisory lock namespace of research reservations.
const LOCK_NAMESPACE: i64 = 71_003_237;

/// A transaction-scoped advisory lock serializing research writes of `key`.
pub(crate) async fn lock(tx: &mut dyn TransactionExecutor, key: &str) -> Result<()> {
	// All Research writers, purges and projection saves use Run row then
	// Research advisory lock. A single order avoids a save/purge deadlock.
	if let Some(run) = key
		.strip_prefix("research-run:")
		.and_then(|id| id.parse::<Uuid>().ok())
	{
		let (sql, values) = Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(run)))
			.lock(LockType::Update)
			.build(PostgresQueryBuilder);
		tx.fetch_one(&sql, convert_values(values)).await?;
	}
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
	#[field(field_type = "text")]
	pub tenant: String,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "research_attempts_run",
		db_index = false,
		on_delete = Cascade,
		on_update = Restrict
	)]
	pub run: ForeignKeyField<crate::apps::execution::models::Run>,
	#[field(field_type = "text")]
	pub invocation_key: String,
	#[field(field_type = "text")]
	pub operation: String,
	#[field(field_type = "text")]
	pub provider: String,
	#[field(field_type = "text")]
	pub destination: String,
	/// The Research account holder's account charged for this attempt.
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
	/// The account whose Research account terms govern the result, for a
	/// provider with such terms (ADR 0058).
	#[field(field_type = "text", null = true)]
	pub retention_account_id: Option<String>,
	/// The `retention_days` of those terms when the result was retrieved.
	#[field(null = true)]
	pub retention_days: Option<i32>,
	/// When the result was replaced with a tombstone; null while it is kept.
	#[field(null = true)]
	pub purged_at: Option<DateTime<Utc>>,
	/// `account_terms`, set together with `purged_at`.
	#[field(field_type = "text", null = true)]
	pub purge_reason: Option<String>,
}

#[model(app_label = "execution", table_name = "research_sources")]
#[derive(Serialize, Deserialize)]
pub struct ResearchSource {
	#[field(primary_key = true, field_type = "uuid")]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "research_sources_run",
		db_index = false,
		on_delete = Cascade,
		on_update = Restrict
	)]
	pub run: ForeignKeyField<crate::apps::execution::models::Run>,
	#[field(field_type = "text")]
	pub url: String,
	#[field(field_type = "text")]
	pub title: String,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	/// The invocation key of the attempt that created the record.
	#[field(field_type = "text", null = true)]
	pub attempt_key: Option<String>,
}

#[model(app_label = "execution", table_name = "research_snapshots")]
#[derive(Serialize, Deserialize)]
pub struct ResearchSnapshot {
	#[field(primary_key = true, field_type = "uuid")]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
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
	/// `retention`, `revocation` or `account_terms`, set together with
	/// `purged_at`.
	#[field(field_type = "text", null = true)]
	pub purge_reason: Option<String>,
	/// The page count of a paginated source such as a PDF.
	#[field(null = true)]
	pub page_count: Option<i32>,
	/// 1-based pages that carry no text (ADR 0053).
	#[field]
	pub pages_without_text: Json<Value>,
	/// Digest and size of the original, recorded when some page carries no text.
	#[field(field_type = "text", null = true)]
	pub original_digest: Option<String>,
	#[field(null = true)]
	pub original_bytes: Option<i64>,
	/// Whether `research_originals` still holds the original.
	#[field]
	pub original_retained: bool,
	/// The invocation key of the attempt that read the page.
	#[field(field_type = "text", null = true)]
	pub attempt_key: Option<String>,
}

/// The original of a snapshot with pages that carry no text (ADR 0053). It is
/// kept apart from the snapshot so that reading a snapshot never loads it.
#[model(app_label = "execution", table_name = "research_originals")]
#[derive(Serialize, Deserialize)]
pub struct ResearchOriginal {
	#[field(primary_key = true, field_type = "uuid")]
	pub snapshot_id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub run_id: Uuid,
	#[field]
	pub bytes: Vec<u8>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

#[model(app_label = "execution", table_name = "research_fragments")]
#[derive(Serialize, Deserialize)]
pub struct ResearchFragment {
	#[field(primary_key = true, field_type = "uuid")]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
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
	/// The page of a page-image fragment (ADR 0053); null for a passage.
	#[field(null = true)]
	pub image_page: Option<i32>,
	#[field(field_type = "text", null = true)]
	pub image_media_type: Option<String>,
	#[field(null = true)]
	pub image_width: Option<i32>,
	#[field(null = true)]
	pub image_height: Option<i32>,
	#[field(field_type = "text", null = true)]
	pub image_digest: Option<String>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	/// `sha256:` digest of the passage as delivered, kept once `text` is gone.
	#[field(field_type = "text", null = true)]
	pub passage_digest: Option<String>,
}

/// The bytes of a page-image Evidence fragment (ADR 0053), kept apart so that
/// reading fragments never loads them.
#[model(app_label = "execution", table_name = "research_page_images")]
#[derive(Serialize, Deserialize)]
pub struct ResearchPageImage {
	#[field(primary_key = true, field_type = "uuid")]
	pub fragment_id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub run_id: Uuid,
	#[field]
	pub bytes: Vec<u8>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

/// The recorded end of one Research account holder's agreement (ADR 0058).
/// It outlives the account's declaration, so removing the declaration never
/// undoes it.
#[model(app_label = "execution", table_name = "research_account_terminations")]
#[derive(Serialize, Deserialize)]
pub struct ResearchAccountTermination {
	#[field(primary_key = true, field_type = "text")]
	pub account_id: String,
	#[field]
	pub terminated_at: DateTime<Utc>,
	#[field]
	pub deletion_after_termination_days: i32,
	#[field(auto_now_add = true)]
	pub recorded_at: DateTime<Utc>,
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

	/// One Run's charges and the attempts whose outcome is not settled:
	/// `(recorded prices of charged attempts, unknown, still dispatched)`.
	pub(crate) async fn run_charges(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
	) -> Result<(u64, u32, u32)> {
		let (sql, values) = Query::select()
			.expr_as(
				Func::coalesce(vec![
					Func::sum(Expr::col(Alias::new("price_micro_usd")).into()),
					Expr::value(0_i64).into(),
				]),
				Alias::new("charged"),
			)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("outcome").ne(Expr::value(AttemptOutcome::Failed.as_str())))
			.build(PostgresQueryBuilder);
		let row = tx.fetch_one(&sql, convert_values(values)).await?;
		let data = QueryRow::from_backend_row(row).data;
		// SUM(bigint) is numeric, which a Run's lifetime total may make wider
		// than i64: a total beyond u64 saturates, and an unreadable one is an
		// error, never a zero charge.
		let total = &data["charged"];
		let charged = total
			.as_u64()
			.or_else(|| {
				total
					.as_str()
					.and_then(|text| text.parse::<u128>().ok())
					.map(|sum| u64::try_from(sum).unwrap_or(u64::MAX))
			})
			.or_else(|| {
				total
					.as_f64()
					.filter(|sum| *sum >= 0.0)
					.map(|sum| sum as u64)
			})
			.ok_or_else(|| Error::External(format!("unreadable research charge total {total}")))?;
		let unknown = count(
			tx,
			Self::table_name(),
			run,
			&[("outcome", AttemptOutcome::Unknown.as_str())],
		)
		.await?;
		let dispatched = count(tx, Self::table_name(), run, &[("outcome", DISPATCHED)]).await?;
		Ok((charged, unknown, dispatched))
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
		// The revocation holds the same lock, so a revoked Run dispatches
		// nothing more even when it was revoked after the caller's own check.
		if ResearchRevocation::exists(tx, run).await? {
			return Ok(Reserved::Revoked);
		}
		if let Some(terms) = &reservation.retention {
			lock(tx, &format!("research-account:{}", terms.account_id)).await?;
			if !ResearchAccountTermination::objects()
				.filter(ResearchAccountTermination::field_account_id().eq(&terms.account_id))
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.is_empty()
			{
				return Ok(Reserved::AccountUnavailable);
			}
		}
		if let Some(attribution) = &reservation.attribution {
			lock(tx, &format!("research-account:{}", attribution.account_id)).await?;
		}
		if Self::find(tx, run, &reservation.key).await?.is_some() {
			return Ok(Reserved::Duplicate);
		}
		let limits = &reservation.limits;
		let maximum = match reservation.operation {
			Operation::Search => limits.search_attempts,
			Operation::Fetch | Operation::Find | Operation::View => limits.fetch_attempts,
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
		let tenant = crate::database::scope::parent_tenant(tx, "runs", "id", run).await?;
		let record = Self::build()
			.tenant(tenant)
			.id(Uuid::now_v7())
			.run(run)
			.invocation_key(reservation.key.clone())
			.operation(reservation.operation.as_str().to_owned())
			.provider(reservation.provider.clone())
			.destination(reservation.destination.clone())
			.account_id(attribution.map(|attribution| attribution.account_id.clone()))
			.price_micro_usd(price)
			.outcome(DISPATCHED.to_owned())
			.result(None)
			.completed_at(None)
			.retention_account_id(
				reservation
					.retention
					.as_ref()
					.map(|terms| terms.account_id.clone()),
			)
			.retention_days(
				reservation
					.retention
					.as_ref()
					.map(|terms| i32::from(terms.retention_days)),
			)
			.purged_at(None)
			.purge_reason(None)
			.finish();
		Self::objects()
			.insert_with_executor(tx, &record)
			.await
			.map_err(FrameworkError::from)?;
		// The Source record of a page read by address holds its slot from now
		// on, under the same Run lock, so nothing is sent without it.
		let source = match &reservation.source_url {
			Some(url) => Some(
				ResearchSource::record(tx, run, &reservation.key, url, "")
					.await?
					.id,
			),
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
		key: &str,
		url: &str,
		title: &str,
	) -> Result<SourceRecord> {
		let tenant = crate::database::scope::parent_tenant(tx, "runs", "id", run).await?;
		let record = Self::build()
			.tenant(tenant)
			.id(Uuid::now_v7())
			.run(run)
			.url(url.to_owned())
			.title(title.to_owned())
			.attempt_key(Some(key.to_owned()))
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
	/// A later account purge is independent of the snapshot's original retention purge.
	pub(crate) async fn account_expired(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		document: Uuid,
	) -> Result<bool> {
		let (sql, values) = Query::select()
			.expr(Expr::value(1_i32))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("id").eq(Expr::value(document)))
			.and_where(
				Expr::col("attempt_key").in_subquery(
					Query::select()
						.column(Alias::new("invocation_key"))
						.from(Alias::new(ResearchAttempt::table_name()))
						.and_where(Expr::col("run_id").eq(Expr::value(run)))
						.and_where(Expr::col("purged_at").is_not_null())
						.and_where(
							Expr::col("purge_reason")
								.eq(Expr::value(PurgeReason::AccountTerms.as_str())),
						)
						.to_owned(),
				),
			)
			.build(PostgresQueryBuilder);
		Ok(tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.is_some())
	}

	pub(crate) async fn record(
		tx: &mut dyn TransactionExecutor,
		key: &str,
		snapshot: &DocumentSnapshot,
	) -> Result<()> {
		let bytes = snapshot
			.lines
			.iter()
			.map(|line| line.text.len())
			.sum::<usize>();
		let tenant =
			crate::database::scope::parent_tenant(tx, "runs", "id", snapshot.run_id).await?;
		let record = Self::build()
			.tenant(tenant)
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
			// The domain bounds the count to the stored range; a larger one is
			// refused rather than recorded smaller than its pages.
			.page_count(
				snapshot
					.page_count
					.map(i32::try_from)
					.transpose()
					.map_err(|_| Error::Invalid("snapshot page count out of range".into()))?,
			)
			.pages_without_text(Json(serde_json::to_value(&snapshot.pages_without_text)?))
			.original_digest(
				snapshot
					.original
					.as_ref()
					.map(|original| original.digest.clone()),
			)
			.original_bytes(
				snapshot
					.original
					.as_ref()
					.map(|original| i64::try_from(original.bytes).unwrap_or(i64::MAX)),
			)
			.original_retained(
				snapshot
					.original
					.as_ref()
					.is_some_and(|original| original.retained),
			)
			.attempt_key(Some(key.to_owned()))
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
					page_count: record
						.page_count
						.and_then(|count| u32::try_from(count).ok()),
					pages_without_text: serde_json::from_value(record.pages_without_text.0)?,
					original: record.original_digest.map(|digest| Original {
						digest,
						bytes: record
							.original_bytes
							.and_then(|bytes| u64::try_from(bytes).ok())
							.unwrap_or(0),
						retained: record.original_retained,
					}),
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
		let tenant =
			crate::database::scope::parent_tenant(tx, "runs", "id", fragment.run_id).await?;
		let record = Self::build()
			.tenant(tenant)
			.id(fragment.reference.0)
			.run_id(fragment.run_id)
			.snapshot_id(fragment.document_id)
			.text(fragment.text.clone())
			.line_start(stored(fragment.line_start))
			.line_end(stored(fragment.line_end))
			.pages(Json(serde_json::to_value(&fragment.pages)?))
			.passage_digest(Some(fragment.passage_digest.clone()))
			.image_page(
				fragment
					.image
					.as_ref()
					.map(|image| i32::try_from(image.page).unwrap_or(i32::MAX)),
			)
			.image_media_type(
				fragment
					.image
					.as_ref()
					.map(|image| image.media_type.clone()),
			)
			.image_width(
				fragment
					.image
					.as_ref()
					.map(|image| i32::try_from(image.width).unwrap_or(i32::MAX)),
			)
			.image_height(
				fragment
					.image
					.as_ref()
					.map(|image| i32::try_from(image.height).unwrap_or(i32::MAX)),
			)
			.image_digest(fragment.image.as_ref().map(|image| image.digest.clone()))
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
				passage_digest: record.passage_digest.unwrap_or_else(|| {
					aidash_domain::research::evidence::passage_digest(&record.text)
				}),
				text: record.text,
				line_start: usize::try_from(record.line_start).unwrap_or(0),
				line_end: usize::try_from(record.line_end).unwrap_or(0),
				pages: serde_json::from_value(record.pages.0)?,
				image: match (record.image_page, record.image_media_type) {
					(Some(page), Some(media_type)) => Some(FragmentImage {
						page: u32::try_from(page).unwrap_or(0),
						media_type,
						width: record
							.image_width
							.and_then(|width| u32::try_from(width).ok())
							.unwrap_or(0),
						height: record
							.image_height
							.and_then(|height| u32::try_from(height).ok())
							.unwrap_or(0),
						digest: record.image_digest.unwrap_or_default(),
					}),
					_ => None,
				},
			});
		}
		Ok(fragments)
	}
}

impl ResearchSnapshot {
	/// Hold snapshot `id` of `run` against a concurrent purge until the
	/// transaction ends. `Err` is why it can no longer be read: a fragment cut
	/// from contents purged meanwhile would outlive them.
	pub(crate) async fn hold(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		id: Uuid,
	) -> Result<std::result::Result<(), Failure>> {
		let (sql, values) = Query::select()
			.column(Alias::new("purge_reason"))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.lock(LockType::Share)
			.build(PostgresQueryBuilder);
		let Some(row) = tx.fetch_optional(&sql, convert_values(values)).await? else {
			return Ok(Err(Failure::UnknownDocument));
		};
		Ok(
			match QueryRow::from_backend_row(row).data["purge_reason"].as_str() {
				None => Ok(()),
				Some(reason) if reason == PurgeReason::Revocation.as_str() => {
					Err(Failure::EvidenceRevoked)
				}
				Some(_) => Err(Failure::SnapshotPurged),
			},
		)
	}

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
			.value(Alias::new("lines"), Value::Array(Vec::new()))
			.value(Alias::new("text_bytes"), 0_i64)
			.value(Alias::new("original_retained"), false)
			.value(Alias::new("purged_at"), at)
			.value(Alias::new("purge_reason"), reason.as_str());
		update
	}
}

impl ResearchOriginal {
	/// Bytes of originals `run` retains; call under the Run's lock.
	pub(crate) async fn held(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<u64> {
		let (sql, values) = Query::select()
			.expr_as(
				Func::coalesce(vec![
					Func::sum(SimpleExpr::FunctionCall(
						"octet_length".into_iden(),
						vec![Expr::col(Alias::new("bytes")).into()],
					)),
					Expr::value(0_i64).into(),
				]),
				Alias::new("held"),
			)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.build(PostgresQueryBuilder);
		let row = tx.fetch_one(&sql, convert_values(values)).await?;
		let data = QueryRow::from_backend_row(row).data;
		Ok(data["held"]
			.as_i64()
			.or_else(|| data["held"].as_str().and_then(|text| text.parse().ok()))
			.and_then(|held| u64::try_from(held).ok())
			.unwrap_or(u64::MAX))
	}

	/// Retain the original of the recorded snapshot `snapshot` of `run`.
	pub(crate) async fn record(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		snapshot: Uuid,
		bytes: &[u8],
	) -> Result<()> {
		let tenant = crate::database::scope::parent_tenant(tx, "runs", "id", run).await?;
		let record = Self::build()
			.tenant(tenant)
			.snapshot_id(snapshot)
			.run_id(run)
			.bytes(bytes.to_vec())
			.finish();
		Self::objects()
			.insert_with_executor(tx, &record)
			.await
			.map_err(FrameworkError::from)?;
		Ok(())
	}

	/// The retained original of snapshot `snapshot` of `run`.
	pub(crate) async fn get(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		snapshot: Uuid,
	) -> Result<Option<Vec<u8>>> {
		Ok(Self::objects()
			.filter(Self::field_run_id().eq(run))
			.filter(Self::field_snapshot_id().eq(snapshot))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.map(|record| record.bytes))
	}

	/// Remove the originals of the snapshots `ids`, as their contents go.
	pub(crate) async fn purge(tx: &mut dyn TransactionExecutor, ids: &[Uuid]) -> Result<u64> {
		if ids.is_empty() {
			return Ok(0);
		}
		let (sql, values) = Query::delete()
			.from_table(Alias::new(Self::table_name()))
			.and_where(Expr::col("snapshot_id").is_in(ids.iter().map(|id| Expr::value(*id))))
			.build(PostgresQueryBuilder);
		Ok(tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected)
	}

	/// Remove every original of `run` for an Evidence Revocation.
	pub(crate) async fn revoke(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<u64> {
		let (sql, values) = Query::delete()
			.from_table(Alias::new(Self::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.build(PostgresQueryBuilder);
		Ok(tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected)
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
		for operation in Operation::ALL {
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

	/// The tombstone `key`'s result becomes once account terms expired its
	/// attempt, which they may do before the attempt settles; `None` while
	/// the attempt keeps its result. Call under the Run's research lock.
	pub(crate) async fn expired_tombstone(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		key: &str,
	) -> Result<Option<Value>> {
		Ok(Self::objects()
			.filter(Self::field_run_id().eq(run))
			.filter(Self::field_invocation_key().eq(key.to_owned()))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.filter(|record| {
				record.purged_at.is_some()
					&& record.purge_reason.as_deref() == Some(PurgeReason::AccountTerms.as_str())
			})
			.and_then(|record| Operation::parse(&record.operation))
			.map(aidash_domain::research::expiry::tombstone))
	}

	/// The tombstone a research result stored now becomes when account terms
	/// already expired the Provider result it carries: its own attempt's, or
	/// the Document snapshot it was built from. Takes the Run's research lock,
	/// so an expiry either finished before and is seen here, or runs after
	/// and replaces what this transaction stores. `None` for other results.
	pub(crate) async fn expired_result(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		key: &str,
		result: &Value,
	) -> Result<Option<Value>> {
		let Some(operation) = aidash_domain::research::revocation::result_operation(result) else {
			return Ok(None);
		};
		lock(tx, &format!("research-run:{run}")).await?;
		if let Some(tombstone) = Self::expired_tombstone(tx, run, key).await? {
			return Ok(Some(tombstone));
		}
		let Some(document) = result["data"]["document_id"]
			.as_str()
			.and_then(|id| id.parse::<Uuid>().ok())
		else {
			return Ok(None);
		};
		let expired = ResearchSnapshot::account_expired(tx, run, document).await?;
		Ok(expired.then(|| aidash_domain::research::expiry::tombstone(operation)))
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
	for operation in Operation::ALL {
		let tombstone = revoked_result(operation);
		let result = || Expr::col(Alias::new("result"));
		let mut update = Query::update();
		// The input names the query or address that was disclosed.
		update
			.table(Alias::new("invocations"))
			.value(Alias::new("result"), tombstone.clone())
			.value(Alias::new("input"), revoked_arguments())
			// Exactly the domain recognizer's test: the contract marker and
			// the operation. A pending approval or a failure has no `limits`,
			// and its input still names what was disclosed.
			.and_where(
				json_text(result(), "contract")
					.eq(Expr::value(aidash_domain::research::RESULT_CONTRACT)),
			)
			.and_where(json_text(result(), "operation").eq(Expr::value(operation.as_str())))
			// A result settled as a tombstone after the revocation still has
			// its input to redact.
			.and_where(
				result().ne(jsonb(&tombstone.to_string())).or(Expr::col(Alias::new("input"))
					.ne(jsonb(&revoked_arguments().to_string()))),
			);
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
	// A call still in flight, or interrupted after its attempt was reserved,
	// has no result to recognize, but its research attempt names it and its
	// input what was disclosed.
	let revoked = revoked_arguments().to_string();
	let attempted = Query::select()
		.column(Alias::new("invocation_key"))
		.from(Alias::new(ResearchAttempt::table_name()))
		.and_where(
			Expr::col((
				Alias::new(ResearchAttempt::table_name()),
				Alias::new("run_id"),
			))
			.equals((Alias::new("invocations"), Alias::new("run_id"))),
		)
		.to_owned();
	let mut update = Query::update();
	update
		.table(Alias::new("invocations"))
		.value(Alias::new("input"), revoked_arguments())
		.and_where(Expr::col(Alias::new("result")).is_null())
		.and_where(Expr::col(Alias::new("input")).ne(jsonb(&revoked)))
		.and_where(Expr::col(Alias::new("idempotency_key")).in_subquery(attempted));
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
	revoke_tool_events(tx, run).await?;
	Ok(u32::try_from(removed).unwrap_or(u32::MAX))
}

fn revoked_arguments() -> Value {
	aidash_domain::context::ContextEvent::revoked_arguments()
}

/// Redact research invocations in the workspace event log, which
/// `/api/events` serves: a `tool.completed` event holds the result and a
/// `tool.started` event the input naming what was disclosed. Those of `run`,
/// or with none those of every revoked Run, so an event appended just after
/// a revocation is caught by the next sweep. Idempotent.
async fn revoke_tool_events(tx: &mut dyn TransactionExecutor, run: Option<Uuid>) -> Result<()> {
	let of_run = || -> SimpleExpr {
		let run_id = json_text(Expr::col(Alias::new("data")), "run_id");
		match run {
			Some(run) => run_id.eq(Expr::value(run.to_string())),
			None => Expr::from(run_id)
				.in_subquery(
					Query::select()
						.expr(Expr::col(Alias::new("run_id")).cast_as_text())
						.from(Alias::new(ResearchRevocation::table_name()))
						.to_owned(),
				)
				.into(),
		}
	};
	let data = || Expr::col(Alias::new("data"));
	for operation in Operation::ALL {
		let tombstone = revoked_result(operation).to_string();
		let result = || json_field(data(), "result");
		let (sql, values) = Query::update()
			.table(Alias::new("events"))
			.value_expr(Alias::new("data"), with_key(data(), "result", &tombstone))
			.and_where(Expr::col("kind").eq("tool.completed"))
			.and_where(
				json_text(Expr::from(result()), "contract")
					.eq(Expr::value(aidash_domain::research::RESULT_CONTRACT)),
			)
			.and_where(
				json_text(Expr::from(result()), "operation").eq(Expr::value(operation.as_str())),
			)
			.and_where(result().ne(jsonb(&tombstone)))
			.and_where(of_run())
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
	}
	// The input of an invocation whose result was redacted, or of an attempt
	// still in flight, which a research attempt records.
	let revoked = revoked_arguments().to_string();
	let events_run = || {
		json_text(
			Expr::col((Alias::new("events"), Alias::new("data"))),
			"run_id",
		)
	};
	let key = || Expr::from(json_text(data(), "idempotency_key"));
	let redacted = Query::select()
		.column(Alias::new("idempotency_key"))
		.from(Alias::new("invocations"))
		.and_where(
			Expr::col(Alias::new("run_id"))
				.cast_as_text()
				.eq(events_run()),
		)
		.and_where(Expr::col(Alias::new("input")).eq(jsonb(&revoked)))
		.to_owned();
	let attempted = Query::select()
		.column(Alias::new("invocation_key"))
		.from(Alias::new(ResearchAttempt::table_name()))
		.and_where(
			Expr::col(Alias::new("run_id"))
				.cast_as_text()
				.eq(events_run()),
		)
		.to_owned();
	let (sql, values) = Query::update()
		.table(Alias::new("events"))
		.value_expr(Alias::new("data"), with_key(data(), "input", &revoked))
		.and_where(Expr::col("kind").eq("tool.started"))
		.and_where(json_field(data(), "input").ne(jsonb(&revoked)))
		.and_where(
			SimpleExpr::from(key().in_subquery(redacted))
				.or(SimpleExpr::from(key().in_subquery(attempted))),
		)
		.and_where(of_run())
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

/// `json -> key`.
pub(crate) fn json_field(json: Expr, key: &str) -> SimpleExpr {
	json.binary(
		reinhardt::query::types::BinOper::PgOperator(
			reinhardt::query::types::PgBinOper::JsonGetByIndex,
		),
		Expr::value(key),
	)
}

/// `json ->> key`.
pub(crate) fn json_text(json: Expr, key: &str) -> SimpleExpr {
	json.binary(
		reinhardt::query::types::BinOper::PgOperator(
			reinhardt::query::types::PgBinOper::JsonGetAsText,
		),
		Expr::value(key),
	)
}

/// A JSON text bound as a `jsonb` value.
pub(crate) fn jsonb(text: &str) -> SimpleExpr {
	Expr::value(text.to_owned()).cast_as("jsonb")
}

/// `json || jsonb_build_object(key, value)`: the object with its top-level
/// `key` set to `value`, as `jsonb_set(json, '{key}', value)` sets it.
pub(crate) fn with_key(json: Expr, key: &str, value: &str) -> SimpleExpr {
	json.binary(
		reinhardt::query::types::BinOper::PgOperator(
			reinhardt::query::types::PgBinOper::Concatenate,
		),
		SimpleExpr::FunctionCall(
			"jsonb_build_object".into_iden(),
			vec![Expr::value(key).into(), jsonb(value)],
		),
	)
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

/// The research attempts of one Run whose Provider results expire together,
/// by invocation key and operation (ADR 0058).
pub(crate) struct ExpiringAttempts {
	pub run: Uuid,
	pub keys: Vec<(String, Operation)>,
}

impl ExpiringAttempts {
	fn keys_of(&self, operation: Operation) -> Vec<String> {
		self.keys
			.iter()
			.filter(|(_, of)| *of == operation)
			.map(|(key, _)| key.clone())
			.collect()
	}

	/// Replace every stored copy of these attempts' Provider results with
	/// tombstones under the Run's research lock, keeping identities, digests
	/// and what Aidash itself sent: the attempts' recorded results, the titles
	/// of the Source records a search created, the contents and originals of
	/// the Document snapshots a read created and the text of their Evidence
	/// fragments, and every stored research result built from either, in
	/// `invocations` and in `tool.completed` events. Attempts and snapshots
	/// are marked purged at `at` unless an earlier purge already marked them.
	/// Returns how many attempts were purged now.
	pub(crate) async fn expire(
		&self,
		tx: &mut dyn TransactionExecutor,
		at: DateTime<Utc>,
	) -> Result<u64> {
		lock(tx, &format!("research-run:{}", self.run)).await?;
		// Revocation dominates later cleanup: a Run revoked after these
		// attempts were chosen keeps its revoked tombstones (ADR 0046).
		if ResearchRevocation::exists(tx, self.run).await? {
			return Ok(0);
		}
		let searches = self.keys_of(Operation::Search);
		let reads = self.keys_of(Operation::Fetch);
		let all = self
			.keys
			.iter()
			.map(|(key, _)| key.clone())
			.collect::<Vec<_>>();
		if !searches.is_empty() {
			let (sql, values) = Query::update()
				.table(Alias::new(ResearchSource::table_name()))
				.value(Alias::new("title"), "")
				.and_where(Expr::col("run_id").eq(Expr::value(self.run)))
				.and_where(
					Expr::col("attempt_key")
						.is_in(searches.iter().map(|key| Expr::value(key.clone()))),
				)
				.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(values)).await?;
		}
		let documents = if reads.is_empty() {
			Vec::new()
		} else {
			ResearchSnapshot::objects()
				.filter(ResearchSnapshot::field_run_id().eq(self.run))
				.filter(
					ResearchSnapshot::field_attempt_key()
						.is_in(reads.into_iter().map(Some).collect::<Vec<_>>()),
				)
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.into_iter()
				.map(|snapshot| snapshot.id)
				.collect::<Vec<_>>()
		};
		if !documents.is_empty() {
			let in_documents =
				|| Expr::col("id").is_in(documents.iter().map(|id| Expr::value(*id)));
			let (sql, values) = ResearchSnapshot::removal(at, PurgeReason::AccountTerms)
				.and_where(in_documents())
				.and_where(Expr::col("purged_at").is_null())
				.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(values)).await?;
			// The page title came from the provider too, and an earlier purge
			// kept it, so it goes from every expired snapshot.
			let (sql, values) = Query::update()
				.table(Alias::new(ResearchSnapshot::table_name()))
				.value(Alias::new("title"), Option::<String>::None)
				.and_where(in_documents())
				.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(values)).await?;
			ResearchOriginal::purge(tx, &documents).await?;
			// Page images were rendered from those originals.
			ResearchPageImage::expire(tx, self.run, &documents).await?;
			// Fragments of a snapshot an earlier purge emptied go too: their
			// passages are copies of what the provider returned.
			let (sql, values) = Query::update()
				.table(Alias::new(ResearchFragment::table_name()))
				.value(Alias::new("text"), "")
				.and_where(Expr::col("run_id").eq(Expr::value(self.run)))
				.and_where(
					Expr::col("snapshot_id").is_in(documents.iter().map(|id| Expr::value(*id))),
				)
				.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(values)).await?;
		}
		// An attempt still dispatched holds no result yet, though its Sources
		// and snapshots may exist. Marked purged, it settles with a tombstone.
		let (sql, values) = Query::update()
			.table(Alias::new(ResearchAttempt::table_name()))
			.value(Alias::new("purged_at"), at)
			.value(
				Alias::new("purge_reason"),
				PurgeReason::AccountTerms.as_str(),
			)
			.and_where(Expr::col("run_id").eq(Expr::value(self.run)))
			.and_where(
				Expr::col("invocation_key").is_in(all.iter().map(|key| Expr::value(key.clone()))),
			)
			.and_where(Expr::col("result").is_null())
			.and_where(Expr::col("purged_at").is_null())
			.build(PostgresQueryBuilder);
		let mut purged = tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected;
		for operation in Operation::ALL {
			let tombstone = aidash_domain::research::expiry::tombstone(operation);
			// The attempts themselves, and any attempt of the Run whose result
			// was built from an expired Document snapshot.
			let mut attempts = Query::update();
			attempts
				.table(Alias::new(ResearchAttempt::table_name()))
				.value(Alias::new("result"), tombstone.clone())
				.value(Alias::new("purged_at"), at)
				.value(
					Alias::new("purge_reason"),
					PurgeReason::AccountTerms.as_str(),
				)
				.and_where(Expr::col("run_id").eq(Expr::value(self.run)))
				.and_where(Expr::col("operation").eq(Expr::value(operation.as_str())))
				.and_where(Expr::col("result").is_not_null())
				.and_where(Expr::col("purged_at").is_null())
				.and_where(produced_by(
					Expr::col(Alias::new("invocation_key")),
					json_text(
						Expr::from(json_field(Expr::col(Alias::new("result")), "data")),
						"document_id",
					),
					&all,
					&documents,
				));
			let (sql, values) = attempts.build(PostgresQueryBuilder);
			purged += tx
				.execute(&sql, convert_values(values))
				.await?
				.rows_affected;
			let (sql, values) = Query::update()
				.table(Alias::new("invocations"))
				.value(Alias::new("result"), tombstone.clone())
				.and_where(Expr::col("run_id").eq(Expr::value(self.run)))
				.and_where(
					json_text(Expr::col(Alias::new("result")), "contract")
						.eq(Expr::value(aidash_domain::research::RESULT_CONTRACT)),
				)
				.and_where(
					json_text(Expr::col(Alias::new("result")), "operation")
						.eq(Expr::value(operation.as_str())),
				)
				.and_where(produced_by(
					Expr::col(Alias::new("idempotency_key")),
					json_text(
						Expr::from(json_field(Expr::col(Alias::new("result")), "data")),
						"document_id",
					),
					&all,
					&documents,
				))
				.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(values)).await?;
			let data = || Expr::col(Alias::new("data"));
			let result = || Expr::from(json_field(data(), "result"));
			let (sql, values) = Query::update()
				.table(Alias::new("events"))
				.value_expr(
					Alias::new("data"),
					with_key(data(), "result", &tombstone.to_string()),
				)
				.and_where(Expr::col("kind").eq("tool.completed"))
				.and_where(json_text(data(), "run_id").eq(Expr::value(self.run.to_string())))
				.and_where(
					json_text(result(), "contract")
						.eq(Expr::value(aidash_domain::research::RESULT_CONTRACT)),
				)
				.and_where(json_text(result(), "operation").eq(Expr::value(operation.as_str())))
				.and_where(produced_by(
					Expr::from(json_text(data(), "idempotency_key")),
					json_text(Expr::from(json_field(result(), "data")), "document_id"),
					&all,
					&documents,
				))
				.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(values)).await?;
		}
		let expired = aidash_domain::research::expiry::ExpiredResults {
			sources: if searches.is_empty() {
				Default::default()
			} else {
				ResearchSource::objects()
					.filter(ResearchSource::field_run_id().eq(self.run))
					.filter(
						ResearchSource::field_attempt_key()
							.is_in(searches.iter().cloned().map(Some).collect::<Vec<_>>()),
					)
					.all_with_executor(tx)
					.await
					.map_err(FrameworkError::from)?
					.into_iter()
					.map(|source| source.id)
					.collect()
			},
			documents: documents.into_iter().collect(),
			through: Some(at),
		};
		ResearchJournalResult::expire(tx, self.run, &all, &expired).await?;
		Ok(purged)
	}
}

/// A stored research result produced by one of `keys`, or built from one of
/// `documents`; false when both are empty.
fn produced_by(key: Expr, document: SimpleExpr, keys: &[String], documents: &[Uuid]) -> SimpleExpr {
	let by_key =
		(!keys.is_empty()).then(|| key.is_in(keys.iter().map(|key| Expr::value(key.clone()))));
	let by_document = (!documents.is_empty()).then(|| {
		Expr::from(document).is_in(documents.iter().map(|id| Expr::value(id.to_string())))
	});
	match (by_key, by_document) {
		(Some(key), Some(document)) => key.or(document),
		(Some(only), None) | (None, Some(only)) => only,
		(None, None) => Expr::value(false).into(),
	}
}

impl ResearchFragment {
	/// Page images delivered to `run`; call under the Run's lock to check the
	/// view limit.
	pub(crate) async fn page_views(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<u32> {
		let (sql, values) = Query::select()
			.expr_as(
				Func::count(Expr::col(Alias::new("id")).into()),
				Alias::new("n"),
			)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("image_page").is_not_null())
			.build(PostgresQueryBuilder);
		let row = tx.fetch_one(&sql, convert_values(values)).await?;
		let n = QueryRow::from_backend_row(row).data["n"]
			.as_i64()
			.unwrap_or(0);
		Ok(u32::try_from(n).unwrap_or(u32::MAX))
	}

	/// The page-image fragment of `page` of snapshot `snapshot` delivered to
	/// `run`, if that page was viewed.
	pub(crate) async fn viewed(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		snapshot: Uuid,
		page: u32,
	) -> Result<Option<EvidenceFragment>> {
		let page = i32::try_from(page).ok();
		let Some(id) = Self::objects()
			.filter(Self::field_run_id().eq(run))
			.filter(Self::field_snapshot_id().eq(snapshot))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.into_iter()
			.find(|record| page.is_some() && record.image_page == page)
			.map(|record| record.id)
		else {
			return Ok(None);
		};
		Ok(Self::delivered(tx, run, &[EvidenceRef(id)]).await?.pop())
	}
}

impl ResearchPageImage {
	/// Keep the bytes of the recorded page-image fragment `fragment` of `run`.
	pub(crate) async fn record(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		fragment: Uuid,
		bytes: &[u8],
	) -> Result<()> {
		let tenant = crate::database::scope::parent_tenant(tx, "runs", "id", run).await?;
		let record = Self::build()
			.tenant(tenant)
			.fragment_id(fragment)
			.run_id(run)
			.bytes(bytes.to_vec())
			.finish();
		Self::objects()
			.insert_with_executor(tx, &record)
			.await
			.map_err(FrameworkError::from)?;
		Ok(())
	}

	/// The bytes of the page-image fragment `fragment` of `run`.
	pub(crate) async fn get(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		fragment: Uuid,
	) -> Result<Option<Vec<u8>>> {
		Ok(Self::objects()
			.filter(Self::field_run_id().eq(run))
			.filter(Self::field_fragment_id().eq(fragment))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.map(|record| record.bytes))
	}

	/// Remove the page images rendered from `run`'s snapshots `documents`,
	/// whose Provider results the Research account terms no longer keep
	/// (ADR 0058). A retention purge keeps them (ADR 0053).
	pub(crate) async fn expire(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		documents: &[Uuid],
	) -> Result<u64> {
		if documents.is_empty() {
			return Ok(0);
		}
		let fragments = Query::select()
			.column(Alias::new("id"))
			.from(Alias::new(ResearchFragment::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("snapshot_id").is_in(documents.iter().map(|id| Expr::value(*id))))
			.to_owned();
		let (sql, values) = Query::delete()
			.from_table(Alias::new(Self::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("fragment_id").in_subquery(fragments))
			.build(PostgresQueryBuilder);
		Ok(tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected)
	}

	/// Remove the page images of `run` for an Evidence Revocation.
	pub(crate) async fn revoke(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<u64> {
		let (sql, values) = Query::delete()
			.from_table(Alias::new(Self::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.build(PostgresQueryBuilder);
		Ok(tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected)
	}
}
