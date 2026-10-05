//! Scoped command reads retain the current native Access and durable replay query.
use crate::authorization::access::Access;
use aidash_application::{Result, ports::authorization::commands::RemoteCommandScope};
use aidash_domain::{Task, identity::commands::Binding, policy::Resource};
use async_trait::async_trait;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, SimpleExpr};
use serde_json::{Value, json};
use uuid::Uuid;
pub(crate) struct Scope<'a> {
	pub(crate) access: &'a mut Access,
	pub(crate) runtime: &'a crate::federation::Federation,
}
#[async_trait]
impl RemoteCommandScope for Scope<'_> {
	async fn binding(&mut self, grant: Uuid) -> Result<Option<Binding>> {
		crate::authorization::remote::execution::binding(self.access, grant)
			.await
			.map(|bound| {
				bound.map(|bound| Binding {
					admission_id: bound.admission_id,
					task_id: bound.task_id,
				})
			})
			.map_err(Into::into)
	}
	async fn task(&mut self, id: Uuid) -> Result<Task> {
		self.access.task_read(id).await.map_err(Into::into)
	}
	async fn previous(&mut self, grant: Uuid, key: &str) -> Result<Option<(String, Value)>> {
		let previous: Option<(String, Value)> = {
			let query_bind_1 = grant;
			let query_bind_2 = key;
			crate::database::native::query_as(
				&Query::select()
					.columns([Alias::new("digest"), Alias::new("result")])
					.from(Alias::new("authorization_remote_commands"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(grant_id=? AND request_key=?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(PostgresQueryBuilder),
			)
			.columns(&["digest", "result"])
			.fetch_optional(&mut **self.access.tx)
			.await?
		};
		Ok(previous)
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.access.task_resource(task).await.map_err(Into::into)
	}
	async fn require_builtin(&mut self, tool: &str) -> Result<()> {
		self.access
			.require(
				&self
					.access
					.resource("tool", format!("builtin:{tool}"), json!({})),
				"tool.invoke",
			)
			.await
			.map_err(Into::into)
	}
}

use reinhardt::query::QueryStatementBuilder as _;

mod effects;
