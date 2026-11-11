//! Output membership and native ordering stay in the caller's exact transaction.
use super::{NativeReads, Reads};
use crate::Result as NativeResult;
use crate::apps::identity::models::DecisionOwner;
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
		if let Some(producers) = self
			.access
			.state_producers(workspace, kind, id)
			.or_else(|| self.access.read_lookups.producers(workspace, kind, id))
		{
			return self
				.access
				.producers_to_check(workspace, producers)
				.await
				.map_err(Into::into);
		}
		let result: NativeResult<Vec<Uuid>> = async {
			let this = &mut *self.access;
			let producers: Vec<Uuid> = {
				let query_bind_1 = workspace;
				let query_bind_2 = kind;
				let query_bind_3 = id;
				crate::database::native::query_scalar(
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
				.scalar_all(&mut **this.tx)
				.await?
			};
			this.producers_to_check(workspace, producers).await
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl OutputScope for Reads<'_> {
	async fn grant_visible(&mut self, grant: Uuid) -> Result<bool> {
		let outermost = self.access.reads_idle();
		let Some(_visit) = self.access.output_visit(grant) else {
			return Ok(true);
		};
		let walk = self
			.access
			.begin_read_walk(DecisionOwner::RemoteGrant(grant), outermost);
		let result = Box::pin(self.access.grant_output_visible(grant)).await;
		self.access
			.end_read_walk(walk, result)
			.await
			.map_err(Into::into)
	}
	async fn remote_grants(&mut self, workspace: Uuid, kind: &str, id: Uuid) -> Result<Vec<Uuid>> {
		if let Some(grants) = self
			.access
			.state_remote_outputs(workspace, kind, id)
			.or_else(|| self.access.read_lookups.remote_grants(workspace, kind, id))
		{
			return Ok(grants);
		}
		let result: NativeResult<Vec<Uuid>> = async {
			let this = &mut *self.access;
			let grants: Vec<Uuid> = {
				let query_bind_1 = workspace;
				let query_bind_2 = kind;
				let query_bind_3 = id;
				crate::database::native::query_scalar(
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
				.scalar_all(&mut **this.tx)
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
		let producers = match self.access.read_lookups.producers(workspace, kind, id) {
			Some(producers) => producers,
			None => {
				crate::apps::identity::models::AuthorizationRunOutput::producers(
					self.access.tx.as_mut(),
					workspace,
					kind,
					id,
				)
				.await?
			}
		};
		self.access
			.producers_to_check(workspace, producers)
			.await
			.map_err(Into::into)
	}
	async fn producer_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		Box::pin(self.access.run_reads_visible(run))
			.await
			.map_err(Into::into)
	}
}
