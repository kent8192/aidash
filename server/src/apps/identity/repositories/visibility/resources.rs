//! Resource adapters retain row membership, native query shape, and authority scope.
use super::{NativeReads, Reads};
use crate::Result as NativeResult;
use aidash_application::{
	Result,
	ports::authorization::visibility::resources::{
		ResourceAccessScope, ResourceEventScope, ResourceVisibilityScope,
	},
};
use aidash_domain::{HumanRequest, Message, Task, policy::Resource};
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
	async fn thread(&mut self, id: Uuid) -> Result<Resource> {
		self.access.thread(id).await.map_err(Into::into)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn output_visible(&mut self, thread: Uuid, kind: &str, id: Uuid) -> Result<bool> {
		self.access
			.output_visible(thread, kind, id)
			.await
			.map_err(Into::into)
	}
	fn cached_human(&self, id: Uuid) -> Option<bool> {
		self.access.cached_humans.get(&id).copied()
	}
	fn remember_human(&mut self, id: Uuid, allowed: bool) {
		self.access.cached_humans.insert(id, allowed);
	}
	async fn humans(&mut self, thread: Uuid, run: Uuid) -> Result<Vec<HumanRequest>> {
		if let Some(requests) = self.access.state_humans(thread, run) {
			return Ok(requests);
		}
		let result: NativeResult<Vec<HumanRequest>> = async {
			let this = &mut *self.access;
			let requests: Vec<HumanRequest> = {
				let query_bind_1 = run;
				let query_bind_2 = thread;
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
										"thread_id",
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
	async fn thread(&mut self, id: Uuid) -> Result<Resource> {
		self.access.thread(id).await.map_err(Into::into)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn output_visible(&mut self, thread: Uuid, kind: &str, id: Uuid) -> Result<bool> {
		self.access
			.output_visible(thread, kind, id)
			.await
			.map_err(Into::into)
	}
	fn cached_human(&self, id: Uuid) -> Option<bool> {
		self.access.cached_human(id)
	}
	fn remember_human(&mut self, id: Uuid, allowed: bool) {
		self.access.remember_human(id, allowed);
	}
	async fn humans(&mut self, thread: Uuid, run: Uuid) -> Result<Vec<HumanRequest>> {
		crate::apps::execution::models::HumanRequest::for_run(self.access.tx.as_mut(), run, thread)
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
	async fn event_task(&mut self, id: Uuid, thread: Option<Uuid>) -> Result<Option<Task>> {
		if let Some(task) = thread.and_then(|thread| self.access.state_task(id, thread)) {
			return Ok(Some(task));
		}
		let result: NativeResult<Option<Task>> = async {
			let this = &mut *self.access;
			let task: Option<Task> = {
				let query_bind_1 = id;
				let query_bind_2 = thread;
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
										"thread_id",
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
	async fn messages_id(&mut self, id: Uuid, thread: Option<Uuid>) -> Result<Vec<Message>> {
		let result: NativeResult<Vec<Message>> = async {
			let this = &mut *self.access;
			let messages: Vec<Message> = {
				let query_bind_1 = id;
				let query_bind_2 = thread;
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
										"thread_id",
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
		thread: Option<Uuid>,
		sender: Option<&str>,
		content: Option<&str>,
	) -> Result<Vec<Message>> {
		let result: NativeResult<Vec<Message>> = async {
			let this = &mut *self.access;
			let messages: Vec<Message> = {
				let query_bind_1 = thread;
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
										"thread_id",
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
