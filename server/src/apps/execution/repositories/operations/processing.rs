//! Housekeeping scans and acknowledgements retain the existing PostgreSQL predicates.
use super::reconciliation::Repository;
use crate::Result as NativeResult;
use aidash_application::{Result, ports::capabilities::processing::OperationProcessingRepository};
use aidash_domain::capabilities::operations::processing::Receipt;
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
impl OperationProcessingRepository for Repository<'_> {
	async fn active_operations(&self) -> Result<Vec<Uuid>> {
		let result: NativeResult<Vec<Uuid>> = async {
			let ids: Vec<Uuid> = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("core_operations"))
					.and_where(Expr::col(Alias::new("state")).is_in([
						"prepared",
						"submitted",
						"running",
						"cancelling",
					]))
					.order_by(Alias::new("updated_at"), Order::Asc)
					.limit(16)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&self.store.pool)
			.await?;
			Ok(ids)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn receipts(&self, after: Uuid) -> Result<Vec<Receipt>> {
		let result: NativeResult<Vec<(Uuid, String, Option<String>)>> = async {
			let rows: Vec<(Uuid, String, Option<String>)> = {
				let query_bind_1 = after;
				sqlx::query_as(
					&Query::select()
						.columns(["id", "digest", "runner_instance"].map(Alias::new))
						.from(Alias::new("core_operations"))
						.and_where(Expr::cust("result->'runner_acknowledged' = 'false'::jsonb"))
						.and_where(Expr::col(Alias::new("state")).is_in([
							"completed",
							"cancelled",
							"failed",
						]))
						.and_where(
							Expr::col(Alias::new("id")).gt(Expr::value(query_bind_1.to_owned())),
						)
						.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
						.limit(16)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&self.store.pool)
				.await?
			};
			Ok(rows)
		}
		.await;
		result
			.map(|rows| {
				rows.into_iter()
					.map(|(id, digest, runner_instance)| Receipt {
						id,
						digest,
						runner_instance,
					})
					.collect()
			})
			.map_err(Into::into)
	}
	async fn mark_acknowledged(&self, id: Uuid) -> Result<()> {
		let result: NativeResult<()> = async {
			{
				let query_bind_1 = id;
				sqlx::query(
					&Query::update()
						.table(Alias::new("core_operations"))
						.value_expr(
							Alias::new("result"),
							Expr::cust("jsonb_set(result, '{runner_acknowledged}', 'true'::jsonb)"),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&self.store.pool)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn storage_blocked(&self, id: Uuid, detail: Value) -> Result<()> {
		let result: NativeResult<()> = async {
			{
				let query_bind_1 = id;
				let query_bind_2 = detail;
				sqlx::query(
					&Query::update()
						.table(Alias::new("core_operations"))
						.value_expr(
							Alias::new("result"),
							SimpleExpr::CustomWithExpr(
								"(jsonb_set(result, '{error}', ?::jsonb))".into(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						)
						.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(Expr::col(Alias::new("state")).is_in([
							"prepared",
							"submitted",
							"running",
							"cancelling",
						]))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&self.store.pool)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn runner_request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
		use aidash_application::ports::capabilities::runner::RunnerTransport as _;
		crate::bootstrap::operation_runner(self.store)?
			.request(method, path, body.as_ref())
			.await
	}
}
