//! Append-only Context Journal entries and pre-I/O Compaction Attempts.
use chrono::{DateTime, Utc};
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// One original Context Journal event; `runs.context` may later truncate it.
#[model(app_label = "execution", table_name = "run_context_events")]
#[derive(Serialize, Deserialize)]
pub struct RunContextEvent {
	#[field(primary_key = true)]
	pub run_id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(primary_key = true)]
	pub seq: i64,
	/// `appended` or `imported` from a projection saved before the journal.
	#[field(field_type = "text")]
	pub origin: String,
	#[field]
	pub event: Json<Value>,
	#[field(field_type = "text")]
	pub digest: String,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

/// A Compaction Attempt recorded before provider I/O; open while `outcome` is null.
#[model(app_label = "execution", table_name = "context_compaction_attempts")]
#[derive(Serialize, Deserialize)]
pub struct ContextCompactionAttempt {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "context_compaction_attempt_run",
		db_index = false,
		on_delete = Cascade,
		on_update = Restrict
	)]
	pub run: ForeignKeyField<crate::apps::execution::models::Run>,
	#[field(field_type = "text")]
	pub stage: String,
	/// NULL is historical unknown. False proves no durable provider dispatch;
	/// true is committed atomically with the Summary Usage Record before I/O.
	#[field(null = true)]
	pub provider_dispatched: Option<bool>,
	#[field(field_type = "text")]
	pub policy_version: String,
	#[field(field_type = "text")]
	pub provider: String,
	#[field]
	pub source_from_seq: i64,
	#[field]
	pub source_through_seq: i64,
	#[field]
	pub base_revision: i64,
	#[field]
	pub observed_input_seq: i64,
	#[field]
	pub before_tokens: i64,
	#[field(field_type = "text", null = true)]
	pub outcome: Option<String>,
	#[field(field_type = "text", null = true)]
	pub reason: Option<String>,
	#[field(field_type = "text", null = true)]
	pub candidate_digest: Option<String>,
	#[field(null = true)]
	pub after_tokens: Option<i64>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	#[field(null = true)]
	pub settled_at: Option<DateTime<Utc>>,
}

impl RunContextEvent {
	/// Provider activity stays inspectable after compaction removes its projection.
	pub(crate) async fn provider_activity_page(
		tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
		run: Uuid,
		offset: u64,
	) -> crate::Result<Vec<aidash_domain::tool::provider_tools::Activity>> {
		use super::research::{json_field, json_text};
		use reinhardt::db::orm::{Model, QueryRow, execution::convert_values};
		use reinhardt::query::{
			Alias, Expr, ExprTrait, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
		};
		let (sql, values) = Query::select()
			.columns([Alias::new("seq"), Alias::new("event")])
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(json_text(Expr::col("event"), "kind").eq(Expr::value("assistant")))
			.and_where(json_field(Expr::col("event"), "provider_activity").is_not_null())
			.order_by(Alias::new("seq"), Order::Desc)
			.offset(offset)
			.limit(100)
			.build(PostgresQueryBuilder);
		tx.fetch_all(&sql, convert_values(values))
			.await?
			.into_iter()
			.map(|row| {
				let row = QueryRow::from_backend_row(row).data;
				let seq = row["seq"].as_u64().ok_or_else(|| {
					crate::Error::Invalid("invalid Context Journal sequence".into())
				})?;
				let event: aidash_domain::context::ContextEvent =
					serde_json::from_value(row["event"].clone())?;
				let aidash_domain::context::ContextEvent::Assistant {
					epoch,
					provider_activity,
					..
				} = event
				else {
					return Err(crate::Error::Invalid(
						"unexpected Provider activity event".into(),
					));
				};
				Ok(aidash_domain::tool::provider_tools::Activity::inspect(
					seq,
					epoch,
					provider_activity,
				))
			})
			.collect()
	}
}
