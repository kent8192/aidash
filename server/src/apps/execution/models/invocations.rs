//! Persistent invocations records.

use crate::apps::execution::serializers::invocations::Invocation as InvocationSummary;
use crate::apps::execution::services::states::InvocationStatus;
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Json, QueryRow};
use reinhardt::model;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[model(app_label = "execution", table_name = "invocations")]
#[derive(Serialize, Deserialize)]
pub struct Invocation {
	#[field(primary_key = true, field_type = "text")]
	pub idempotency_key: String,
	#[field(field_type = "text")]
	pub tenant: String,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "invocations_run_id_fkey",
		db_index = false
	)]
	pub run: ForeignKeyField<crate::apps::execution::models::Run>,
	#[field(field_type = "text")]
	pub tool: String,
	#[field]
	pub input: Json<Value>,
	#[field(field_type = "text", max_length = 64)]
	pub status: InvocationStatus,
	#[field(null = true)]
	pub result: Option<Json<Value>>,
	#[field]
	pub replay_safe: bool,
	/// The Tenant Secret this call carried and its pinned version. The value is
	/// never recorded. Null for a call without one.
	#[field(null = true)]
	pub tenant_secret_id: Option<Uuid>,
	#[field(null = true, field_type = "text")]
	pub tenant_secret_version: Option<String>,
	#[field(null = true, field_type = "text")]
	pub tenant_secret_host: Option<String>,
	/// OAuth Connection used for this invocation, never its bearer.
	#[field(null = true)]
	pub connection_id: Option<Uuid>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

impl Invocation {
	pub(crate) async fn completed(
		db: &mut dyn TransactionExecutor,
		run: Uuid,
		key: &str,
		name: &str,
		input: &Value,
	) -> Result<Option<Value>> {
		let (sql, values) = Query::select()
			.column(reinhardt::query::ColumnRef::Asterisk)
			.from(Alias::new("invocations"))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("idempotency_key").eq(Expr::value(key)))
			.build(PostgresQueryBuilder);
		let Some(row) = db.fetch_optional(&sql, convert_values(values)).await? else {
			return Ok(None);
		};
		let saved: InvocationSummary =
			serde_json::from_value(QueryRow::from_backend_row(row).data)?;
		if saved.tool != name || saved.input != *input {
			return Err(Error::Conflict(
				"tool idempotency key reused with different input".into(),
			));
		}
		if saved.status == "COMPLETED" {
			saved
				.result
				.map(Some)
				.ok_or_else(|| Error::Conflict("completed invocation has no result".into()))
		} else {
			Ok(None)
		}
	}
}

impl Invocation {}
