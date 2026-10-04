//! Tool ports retain the exact Access mutex guard through authorization and disclosure.
use crate::{
	Result as NativeResult,
	apps::identity::services::{access::Access, catalog},
	federation::Federation,
};
use aidash_application::{
	Result,
	ports::authorization::tools::{AgentToolRepository, AgentToolScope},
};
use aidash_domain::{
	HumanRequest, Run, RunMetadata, Task,
	policy::Resource,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard};
use uuid::Uuid;

pub(crate) struct AgentTools<'a> {
	pub(crate) remote: Option<&'a Federation>,
	pub(crate) access: &'a Arc<Mutex<Access>>,
	pub(crate) run: &'a Run,
}
struct Scope<'a> {
	remote: Option<&'a Federation>,
	access: OwnedMutexGuard<Access>,
	run: &'a Run,
}
#[async_trait]
impl AgentToolRepository for AgentTools<'_> {
	fn is_remote(&self) -> bool {
		self.remote.is_some()
	}
	async fn lease(&self) -> Result<Box<dyn AgentToolScope + '_>> {
		Ok(Box::new(Scope {
			remote: self.remote,
			access: self.access.clone().lock_owned().await,
			run: self.run,
		}))
	}
}
#[async_trait]
impl AgentToolScope for Scope<'_> {
	fn transaction_active(&self) -> bool {
		self.access.tx.is_active()
	}
	async fn suspend(&mut self) -> Result<()> {
		self.access.suspend().await.map_err(Into::into)
	}
	async fn replace_remote_authority(&mut self) -> Result<bool> {
		let Some(f) = self.remote else {
			return Ok(false);
		};
		let fresh =
			super::super::services::peer::admission::worker_lease(f, &self.run.metadata()).await?;
		let Some((fresh, _)) = fresh else {
			return Ok(false);
		};
		*self.access = fresh;
		Ok(true)
	}
	fn context(&self) -> &Value {
		&self.access.context
	}
	fn subjects(&self) -> &[String] {
		&self.access.subjects
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn catalog(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		catalog::entry(&mut self.access, reference, action)
			.await
			.map_err(Into::into)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn task_read(&mut self, task: Uuid) -> Result<Task> {
		self.access.task_read(task).await.map_err(Into::into)
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.access.task_resource(task).await.map_err(Into::into)
	}
	async fn artifact_creation_resource(&mut self, task: Uuid, creator: &str) -> Result<Resource> {
		self.access
			.artifact_creation_resource(task, creator)
			.await
			.map_err(Into::into)
	}
	async fn memory_resource(&mut self, run: &RunMetadata) -> Result<Resource> {
		self.access
			.memory_resource(run.clone())
			.await
			.map_err(Into::into)
	}
	async fn human_request(&mut self, run: &RunMetadata, id: Uuid) -> Result<Option<HumanRequest>> {
		let result: NativeResult<Option<HumanRequest>> = async {
			Ok({
				let query_bind_1 = id;
				let query_bind_2 = run.id;
				let query_bind_3 = run.workspace_id;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("human_requests"))
						.and_where(
							Condition::all()
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id")))
										.eq(SimpleExpr::CustomWithExpr(
											"(?)".to_owned(),
											vec![Expr::value(query_bind_1.to_owned()).into()],
										)),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"run_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"workspace_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_3.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn human_resource(&mut self, request: &HumanRequest) -> Result<Resource> {
		self.access
			.human_resource(request)
			.await
			.map_err(Into::into)
	}
}

use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait as _, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
