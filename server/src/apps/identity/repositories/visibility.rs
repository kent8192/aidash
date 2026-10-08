//! Read adapters preserve the original query trees and the caller's live authority.
use crate::apps::execution::generation::repositories::contracts::RequestAccess;
use crate::{
	Result as NativeResult,
	apps::identity::services::access::{Access, NativeAccess},
};
use aidash_application::{
	Result,
	ports::authorization::visibility::{
		EventVisibilityScope, LocalRunVisibilityScope, RunVisibilityScope,
	},
};
use aidash_domain::{
	Conversation, Event, HumanRequest, RunMetadata, Task, generation::requests::Request,
	policy::Resource,
};
use async_trait::async_trait;
use reinhardt::query::ColumnRef::Asterisk;
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait as _, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;

pub(crate) struct Reads<'a> {
	pub access: &'a mut Access,
}
pub(crate) struct NativeReads<'a> {
	pub access: &'a mut NativeAccess,
}
#[async_trait]
impl LocalRunVisibilityScope for Reads<'_> {
	fn cached(&self, run: &RunMetadata) -> Option<bool> {
		self.access
			.dependency_frontier
			.is_none()
			.then(|| {
				self.access
					.cached_runs
					.get(&(run.workspace_id, run.id))
					.copied()
			})
			.flatten()
	}
	fn remember(&mut self, run: &RunMetadata, allowed: bool) {
		if self.access.dependency_frontier.is_none() {
			self.access
				.cached_runs
				.insert((run.workspace_id, run.id), allowed);
		}
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	async fn memory_resource(&mut self, run: &RunMetadata, _: &Resource) -> Result<Resource> {
		self.access.memory_resource(run).await.map_err(Into::into)
	}
	async fn task_visible(&mut self, task: &Task) -> Result<bool> {
		self.access.task_visible(task).await.map_err(Into::into)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn human_reads(&mut self, workspace: Uuid, run: Uuid) -> Result<bool> {
		self.access
			.human_reads(workspace, run)
			.await
			.map_err(Into::into)
	}
	async fn run_reads(&mut self, run: Uuid) -> Result<bool> {
		self.access.run_reads_visible(run).await.map_err(Into::into)
	}
	async fn task(&mut self, run: &RunMetadata) -> Result<Option<Task>> {
		let result: NativeResult<Option<Task>> = async {
			let this = &mut *self.access;
			let task: Option<Task> = {
				let query_bind_1 = run.task_id;
				let query_bind_2 = run.workspace_id;
				aidash_server::database::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("tasks"))
						.cond_where(
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
										"workspace_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **this.tx)
				.await?
			};
			Ok(task)
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl RunVisibilityScope for Reads<'_> {
	fn node_id(&self) -> &str {
		&self.access.node_id
	}
	async fn foreign_base_visible(&mut self, run: &RunMetadata) -> Result<bool> {
		self.access
			.foreign_run_base_visible(run)
			.await
			.map_err(Into::into)
	}
	async fn scoped_admission(&mut self, id: Uuid) -> Result<bool> {
		let result: NativeResult<bool> = async {
			let this = &mut *self.access;
			let admission: Option<Uuid> = {
				let query_bind_1 = id;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("authorization_remote_admissions"))
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
				.scalar_optional(&mut **this.tx)
				.await?
			};
			Ok(admission.is_some())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn legacy_execution(&mut self, run: &RunMetadata) -> Result<bool> {
		let result: NativeResult<bool> = async {
			let this = &mut *self.access;
			let local: Option<Uuid> = {
				let query_bind_1 = run.id;
				let query_bind_2 = run.task_id;
				let query_bind_3 = run.workspace_id;
				let query_bind_4 = &this.identity.tenant;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("run_id"))
						.from(Alias::new("authorization_execution"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(run_id=? AND task_id=? AND workspace_id=? AND tenant=?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
								Expr::value(query_bind_3.to_owned()).into(),
								Expr::value(query_bind_4.to_owned()).into(),
							],
						))
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut **this.tx)
				.await?
			};
			Ok(local.is_some())
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl LocalRunVisibilityScope for NativeReads<'_> {
	fn cached(&self, run: &RunMetadata) -> Option<bool> {
		self.access.cached_run(run)
	}
	fn remember(&mut self, run: &RunMetadata, allowed: bool) {
		self.access.remember_run(run, allowed);
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	async fn memory_resource(&mut self, run: &RunMetadata, _: &Resource) -> Result<Resource> {
		self.access.memory_resource(run).await.map_err(Into::into)
	}

	async fn task(&mut self, run: &RunMetadata) -> Result<Option<Task>> {
		crate::apps::workspaces::models::Task::read_in(
			self.access.tx.as_mut(),
			run.task_id,
			run.workspace_id,
		)
		.await
		.map_err(Into::into)
	}
	async fn task_visible(&mut self, task: &Task) -> Result<bool> {
		self.access.task_visible(task).await.map_err(Into::into)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn human_reads(&mut self, workspace: Uuid, run: Uuid) -> Result<bool> {
		self.access
			.human_reads(workspace, run)
			.await
			.map_err(Into::into)
	}
	async fn run_reads(&mut self, run: Uuid) -> Result<bool> {
		self.access.run_reads_visible(run).await.map_err(Into::into)
	}
}
#[async_trait]
impl EventVisibilityScope for Reads<'_> {
	async fn marketplace_visible(&mut self, event: &Event) -> Result<bool> {
		let node = self.access.node_id.clone();
		crate::marketplace::events::visible(self.access, event, &node)
			.await
			.map_err(Into::into)
	}
	async fn resource_visible(&mut self, event: &Event) -> Result<Option<bool>> {
		self.access
			.resource_event_visible(event)
			.await
			.map_err(Into::into)
	}
	async fn generation_visible(&mut self, job: &Request) -> Result<bool> {
		job.visible(self.access).await.map_err(Into::into)
	}
	async fn conversation_resource(&mut self, conversation: &Conversation) -> Result<Resource> {
		self.access
			.conversation_resource(conversation)
			.await
			.map_err(Into::into)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn human_visible(&mut self, request: &HumanRequest) -> Result<bool> {
		self.access.human_visible(request).await.map_err(Into::into)
	}
	async fn run_visible(&mut self, run: &RunMetadata) -> Result<bool> {
		self.access.run_visible(run).await.map_err(Into::into)
	}
	async fn generation(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<Option<Request>> {
		let result: NativeResult<Option<Request>> = async {
			let this = &mut *self.access;
			let job: Option<crate::generation::Request> = {
				let query_bind_1 = id;
				let query_bind_2 = &this.identity.tenant;
				let query_bind_3 = workspace;
				crate::database::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("generation_requests"))
						.cond_where(
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
										"tenant",
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
				.fetch_optional(&mut **this.tx)
				.await?
			};
			Ok(job)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn conversation(
		&mut self,
		id: Uuid,
		workspace: Option<Uuid>,
	) -> Result<Option<Conversation>> {
		let result: NativeResult<Option<Conversation>> = async {
			let this = &mut *self.access;
			let conversation: Option<Conversation> = {
				let query_bind_1 = id;
				let query_bind_2 = workspace;
				aidash_server::database::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("conversations"))
						.cond_where(
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
										"workspace_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **this.tx)
				.await?
			};
			Ok(conversation)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn human(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<Option<HumanRequest>> {
		let result: NativeResult<Option<HumanRequest>> = async {
			let this = &mut *self.access;
			let request: Option<HumanRequest> = {
				let query_bind_1 = id;
				let query_bind_2 = workspace;
				aidash_server::database::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("human_requests"))
						.cond_where(
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
										"workspace_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **this.tx)
				.await?
			};
			Ok(request)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn run(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<Option<RunMetadata>> {
		let result: NativeResult<Option<RunMetadata>> = async {
			let this = &mut *self.access;
			let run: Option<crate::domain::RunMetadata> = {
				let query_bind_1 = id;
				let query_bind_2 = workspace;
				aidash_server::database::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("runs"))
						.cond_where(
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
										"workspace_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **this.tx)
				.await?
			};
			Ok(run)
		}
		.await;
		result.map_err(Into::into)
	}
}

pub(crate) mod resources;

pub(crate) mod provenance;

pub(crate) mod generation;

pub(crate) mod outputs;

pub(crate) mod catalog;
pub(crate) mod semantic;

pub(crate) mod registry_reads;

pub(crate) mod workspaces;

pub mod projection;

pub mod journals;

pub mod stream;

pub mod records;

pub mod state;
