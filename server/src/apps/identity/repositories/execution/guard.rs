//! Native guard reads preserve the worker's Access connection and registry locks.
use crate::{
	Result as NativeResult,
	apps::identity::services::{access::Access, catalog},
	federation::Federation,
};
use aidash_application::{Result, ports::authorization::guard::RunGuardScope};
use aidash_domain::{
	RunMetadata, Task,
	policy::{PolicyBundle, Resource},
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait as _, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use uuid::Uuid;
pub(crate) struct RunGuard<'a> {
	pub(crate) federation: &'a Federation,
	pub(crate) access: &'a mut Access,
}
#[async_trait]
impl RunGuardScope for RunGuard<'_> {
	fn node_id(&self) -> &str {
		&self.federation.config.node_id
	}
	fn bundle(&self) -> &PolicyBundle {
		&self.access.snapshot.bundle
	}
	async fn run_visible(&mut self, run: &RunMetadata) -> Result<bool> {
		self.access.run_visible(run).await.map_err(Into::into)
	}
	async fn cluster_targets(&mut self, workspace: Uuid) -> Result<Vec<String>> {
		let result: NativeResult<Vec<String>> = async {
			Ok({
				let query_bind_1 = workspace;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("target"))
						.from(Alias::new("conversations"))
						.cond_where(
							Condition::all()
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"workspace_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_1.to_owned()).into()],
									)),
								)
								.add(Expr::cust("target_kind='cluster'")),
						)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_all(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn catalog(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		catalog::entry(self.access, reference, action)
			.await
			.map_err(Into::into)
	}
	async fn task_read(&mut self, task: Uuid) -> Result<Task> {
		self.access.task_read(task).await.map_err(Into::into)
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.access.task_resource(task).await.map_err(Into::into)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn require_live(&mut self, task: Uuid, agent: &EntityRef) -> Result<()> {
		crate::generation::provision::require_live(
			self.access,
			&self.federation.config.node_id,
			task,
			agent,
		)
		.await
		.map_err(Into::into)
	}
	async fn check_pinned(&mut self, entry: &Entry) -> Result<()> {
		crate::marketplace::check_pinned(self.access, entry)
			.await
			.map_err(Into::into)
	}
	async fn context_authority(&mut self, run: &RunMetadata) -> Result<()> {
		crate::capabilities::sessions::context_authority(self.access, run)
			.await
			.map_err(Into::into)
	}
}
