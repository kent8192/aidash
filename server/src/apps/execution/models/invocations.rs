//! Persistent invocations records.

use crate::Result;
use crate::apps::execution::serializers::invocations::Invocation as InvocationSummary;
use crate::apps::execution::services::states::InvocationStatus;
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Json, QueryRow};
use reinhardt::model;
use reinhardt::query::{Alias, Expr, Order, PostgresQueryBuilder, Query, SelectStatement};
use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[model(app_label = "execution", table_name = "invocations")]
#[derive(Serialize, Deserialize)]
pub struct Invocation {
	#[field(primary_key = true, field_type = "text")]
	pub idempotency_key: String,
	#[field]
	pub run_id: uuid::Uuid,
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
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

impl Invocation {
	pub(crate) async fn page(
		db: &mut dyn TransactionExecutor,
		run: Uuid,
		offset: u64,
	) -> Result<Vec<InvocationSummary>> {
		let (sql, values) = summary(None)
			.from(Alias::new("invocations"))
			.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run)))
			.order_by(Alias::new("created_at"), Order::Asc)
			.order_by(Alias::new("idempotency_key"), Order::Asc)
			.limit(100)
			.offset(offset)
			.build(PostgresQueryBuilder);
		db.fetch_all(&sql, convert_values(values))
			.await?
			.into_iter()
			.map(|row| {
				Ok(serde_json::from_value(
					QueryRow::from_backend_row(row).data,
				)?)
			})
			.collect()
	}
}

/// Bound previews in the database so oversized durable values never have to
/// be loaded merely to render a journal page.
pub(crate) fn summary(alias: Option<&str>) -> SelectStatement {
	let mut query = Query::select();
	for column in [
		"idempotency_key",
		"run_id",
		"tool",
		"status",
		"replay_safe",
		"created_at",
	] {
		if let Some(table) = alias {
			query.column((Alias::new(table), Alias::new(column)));
		} else {
			query.column(Alias::new(column));
		}
	}
	for column in ["input", "result"] {
		let name = alias.map_or_else(|| column.to_owned(), |table| format!("{table}.{column}"));
		query.expr_as(Expr::cust(format!("CASE WHEN octet_length({name}::text) > 1024 THEN jsonb_build_object('truncated',true,'preview',left({name}::text,1024)) ELSE {name} END")), Alias::new(column));
	}
	query
}

impl Invocation {}
