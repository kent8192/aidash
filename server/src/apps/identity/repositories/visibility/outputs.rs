//! Output membership and native ordering stay in the caller's exact transaction.
use super::{NativeReads, Reads};
use crate::Result as NativeResult;
use aidash_application::{
	Result,
	ports::authorization::visibility::provenance::{LocalOutputScope, OutputScope},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, Order, PostgresQueryBuilder, Query, QueryStatementBuilder as _, SimpleExpr,
};
use uuid::Uuid;
#[async_trait]
impl LocalOutputScope for Reads<'_> {
	async fn producer_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		Box::pin(self.access.run_reads_visible(run))
			.await
			.map_err(Into::into)
	}
	async fn producers(&mut self, workspace: Uuid, kind: &str, id: Uuid) -> Result<Vec<Uuid>> {
		let result: NativeResult<Vec<Uuid>> = async {
			let this = &mut *self.access;
			let producers: Vec<Uuid> = {
				let query_bind_1 = workspace;
				let query_bind_2 = kind;
				let query_bind_3 = id;
				sqlx::query_scalar(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::Alias::new("run_id")),
						))
						.from(reinhardt::query::Alias::new("authorization_run_outputs"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(workspace_id = ? AND resource_kind = ? AND resource_id = ?)"
								.to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
								Expr::value(query_bind_3.to_owned()).into(),
							],
						))
						.order_by_expr(
							reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
								reinhardt::query::Alias::new("run_id"),
							)),
							reinhardt::query::Order::Asc,
						)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.fetch_all(&mut **this.tx)
				.await?
			};
			Ok(producers)
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl OutputScope for Reads<'_> {
	async fn grant_visible(&mut self, grant: Uuid) -> Result<bool> {
		let Some(_visit) = self.access.output_visit(grant) else {
			return Ok(true);
		};
		Box::pin(self.access.grant_output_visible(grant))
			.await
			.map_err(Into::into)
	}
	async fn remote_grants(&mut self, workspace: Uuid, kind: &str, id: Uuid) -> Result<Vec<Uuid>> {
		let result: NativeResult<Vec<Uuid>> = async {
			let this = &mut *self.access;
			let grants: Vec<Uuid> = {
				let query_bind_1 = workspace;
				let query_bind_2 = kind;
				let query_bind_3 = id;
				sqlx::query_scalar(
					&Query::select()
						.column(Alias::new("grant_id"))
						.from(Alias::new("authorization_remote_outputs"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(workspace_id=? AND resource_kind=? AND resource_id=?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
								Expr::value(query_bind_3.to_owned()).into(),
							],
						))
						.order_by(Alias::new("grant_id"), Order::Asc)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **this.tx)
				.await?
			};
			Ok(grants)
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl LocalOutputScope for NativeReads<'_> {
	async fn producers(&mut self, workspace: Uuid, kind: &str, id: Uuid) -> Result<Vec<Uuid>> {
		crate::apps::identity::models::AuthorizationRunOutput::producers(
			self.access.tx.as_mut(),
			workspace,
			kind,
			id,
		)
		.await
		.map_err(Into::into)
	}
	async fn producer_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		Box::pin(self.access.run_reads_visible(run))
			.await
			.map_err(Into::into)
	}
}
