//! Durable tool attempts share their run's worker fence and event transaction.
use super::Invocation;
use crate::Result;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{Model, execution::convert_values};
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::Value;
use uuid::Uuid;

impl Invocation {
	pub(crate) async fn complete(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		key: &str,
		result: &Value,
	) -> Result<bool> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("status"), Expr::value("COMPLETED"))
			.value_expr(Alias::new("result"), Expr::value(result.clone()))
			.and_where(Expr::col("idempotency_key").eq(Expr::value(key)))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("status").ne(reinhardt::query::Expr::value("COMPLETED")))
			.build(PostgresQueryBuilder);
		Ok(tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected
			== 1)
	}
}
