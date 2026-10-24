//! Resource adapters retain row membership, native query shape, and authority scope.
use super::{NativeReads, Reads};
use crate::Result as NativeResult;
use aidash_application::{
	Result,
	ports::authorization::visibility::resources::{
		ResourceAccessScope, ResourceEventScope, ResourceVisibilityScope,
	},
};
use aidash_domain::{Artifact, HumanRequest, Message, Task, policy::Resource};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait as _, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
impl ResourceVisibilityScope for Reads<'_> {
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn output_visible(&mut self, workspace: Uuid, kind: &str, id: Uuid) -> Result<bool> {
		self.access
			.output_visible(workspace, kind, id)
			.await
			.map_err(Into::into)
	}
	fn cached_human(&self, id: Uuid) -> Option<bool> {
		self.access.cached_humans.get(&id).copied()
	}
	fn remember_human(&mut self, id: Uuid, allowed: bool) {
		self.access.cached_humans.insert(id, allowed);
	}
	async fn artifact_task(&mut self, artifact: &Artifact) -> Result<Option<Task>> {
		let result: NativeResult<Option<Task>> = async {
			let this = &mut *self.access;
			let task: Option<Task> = {
				let query_bind_1 = artifact.task_id;
				let query_bind_2 = artifact.workspace_id;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("tasks"))
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
	async fn humans(&mut self, workspace: Uuid, run: Uuid) -> Result<Vec<HumanRequest>> {
		let result: NativeResult<Vec<HumanRequest>> = async {
			let this = &mut *self.access;
			let requests: Vec<HumanRequest> = {
				let query_bind_1 = run;
				let query_bind_2 = workspace;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("human_requests"))
						.and_where(
							Condition::all()
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"run_id",
									)))
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
				.fetch_all(&mut **this.tx)
				.await?
			};
			Ok(requests)
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl ResourceVisibilityScope for NativeReads<'_> {
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn output_visible(&mut self, workspace: Uuid, kind: &str, id: Uuid) -> Result<bool> {
		self.access
			.output_visible(workspace, kind, id)
			.await
			.map_err(Into::into)
	}
	fn cached_human(&self, id: Uuid) -> Option<bool> {
		self.access.cached_human(id)
	}
	fn remember_human(&mut self, id: Uuid, allowed: bool) {
		self.access.remember_human(id, allowed);
	}
	async fn artifact_task(&mut self, artifact: &Artifact) -> Result<Option<Task>> {
		crate::apps::workspaces::models::Task::read_in(
			self.access.tx.as_mut(),
			artifact.task_id,
			artifact.workspace_id,
		)
		.await
		.map_err(Into::into)
	}
	async fn humans(&mut self, workspace: Uuid, run: Uuid) -> Result<Vec<HumanRequest>> {
		crate::apps::execution::models::HumanRequest::for_run(
			self.access.tx.as_mut(),
			run,
			workspace,
		)
		.await
		.map_err(Into::into)
	}
}
#[async_trait]
impl ResourceAccessScope for Reads<'_> {
	async fn task_any(&mut self, id: Uuid) -> Result<Option<Task>> {
		let result: NativeResult<Option<Task>> = async {
			let this = &mut *self.access;
			let task: Option<Task> = {
				let query_bind_1 = id;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("tasks"))
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
impl ResourceEventScope for Reads<'_> {
	async fn event_task(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<Option<Task>> {
		let result: NativeResult<Option<Task>> = async {
			let this = &mut *self.access;
			let task: Option<Task> = {
				let query_bind_1 = id;
				let query_bind_2 = workspace;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("tasks"))
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
	async fn artifact(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<Option<Artifact>> {
		let result: NativeResult<Option<Artifact>> = async {
			let this = &mut *self.access;
			let artifact: Option<Artifact> = {
				let query_bind_1 = id;
				let query_bind_2 = workspace;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("artifacts"))
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
			Ok(artifact)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn messages_id(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<Vec<Message>> {
		let result: NativeResult<Vec<Message>> = async {
			let this = &mut *self.access;
			let messages: Vec<Message> = {
				let query_bind_1 = id;
				let query_bind_2 = workspace;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("messages"))
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
				.fetch_all(&mut **this.tx)
				.await?
			};
			Ok(messages)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn messages_legacy(
		&mut self,
		workspace: Option<Uuid>,
		sender: Option<&str>,
		content: Option<&str>,
	) -> Result<Vec<Message>> {
		let result: NativeResult<Vec<Message>> = async {
			let this = &mut *self.access;
			let messages: Vec<Message> = {
				let query_bind_1 = workspace;
				let query_bind_2 = sender;
				let query_bind_3 = content;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("messages"))
						.and_where(
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
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"sender",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"content",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_3.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **this.tx)
				.await?
			};
			Ok(messages)
		}
		.await;
		result.map_err(Into::into)
	}
}
