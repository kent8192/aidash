//! Run-scoped Research evidence: provider attempts with their cost attribution,
//! Source records, Document snapshots and delivered Evidence fragments. Every
//! read filters by Run, so another Run's identifier is simply absent.
use crate::Result;
use aidash_application::ports::research::{Reservation, Reserved};
use aidash_domain::research::{
	Operation,
	citations::EvidenceRef,
	document::{Completeness, Line},
	evidence::{AttemptOutcome, AttemptState, DocumentSnapshot, EvidenceFragment, SourceRecord},
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

async fn lock(tx: &mut dyn TransactionExecutor, key: &str) -> Result<()> {
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
	#[field]
	pub lines: Json<Value>,
	#[field]
	pub text_bytes: i64,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
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

fn stored(value: usize) -> i32 {
	i32::try_from(value).unwrap_or(i32::MAX)
}

async fn count(
	tx: &mut dyn TransactionExecutor,
	table: &str,
	run: Uuid,
	operation: Option<&str>,
) -> Result<u32> {
	let mut query = Query::select();
	query
		.expr_as(
			Func::count(Expr::col(Alias::new("id")).into()),
			Alias::new("n"),
		)
		.from(Alias::new(table))
		.and_where(Expr::col("run_id").eq(Expr::value(run)));
	if let Some(operation) = operation {
		query.and_where(Expr::col("operation").eq(Expr::value(operation)));
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
		count(tx, Self::table_name(), run, Some(operation.as_str())).await
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
					Func::sum(Expr::col(Alias::new("price_micro_usd")).into()).into(),
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
		if Self::count(tx, run, reservation.operation).await? >= reservation.max_attempts {
			return Ok(Reserved::AttemptLimit);
		}
		let attribution = reservation.attribution.as_ref();
		if let (Some(attribution), Some(budget)) = (attribution, &reservation.budget) {
			let spent = Self::spend(tx, &attribution.account_id, budget.since).await?;
			if spent.saturating_add(attribution.price_micro_usd) > budget.limit_micro_usd {
				return Ok(Reserved::BudgetExhausted);
			}
		}
		let record =
			Self::build()
				.id(Uuid::now_v7())
				.run_id(run)
				.invocation_key(reservation.key.clone())
				.operation(reservation.operation.as_str().to_owned())
				.provider(reservation.provider.clone())
				.destination(reservation.destination.clone())
				.account_id(attribution.map(|attribution| attribution.account_id.clone()))
				.price_micro_usd(attribution.map(|attribution| {
					i64::try_from(attribution.price_micro_usd).unwrap_or(i64::MAX)
				}))
				.outcome(DISPATCHED.to_owned())
				.result(None)
				.completed_at(None)
				.finish();
		Self::objects()
			.insert_with_executor(tx, &record)
			.await
			.map_err(FrameworkError::from)?;
		Ok(Reserved::Reserved)
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
		count(tx, Self::table_name(), run, None).await
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
				})
			})
			.transpose()
	}

	/// Snapshot count and retained text bytes of one Run.
	pub(crate) async fn usage(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<(u32, u64)> {
		let (sql, values) = Query::select()
			.expr_as(
				Func::count(Expr::col(Alias::new("id")).into()),
				Alias::new("n"),
			)
			.expr_as(
				Func::coalesce(vec![
					Func::sum(Expr::col(Alias::new("text_bytes")).into()).into(),
					Expr::value(0_i64).into(),
				]),
				Alias::new("bytes"),
			)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
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
