//! Native effects reuse the original SQL, locks and Store transaction methods.
use super::Scope;
use crate::Result as NativeResult;
use aidash_application::{
	Result,
	ports::authorization::commands::effects::{
		Applied, Effect, RemoteCommandEffects, WriteContext,
	},
};
use aidash_domain::{Message, policy::Resource};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef::Asterisk, Expr, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::{Value, json};
use uuid::Uuid;
#[async_trait]
impl RemoteCommandEffects for Scope<'_> {
	fn local_node(&self) -> &str {
		&self.runtime.config.node_id
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	async fn artifact_creation_resource(&mut self, task: Uuid, owner: &str) -> Result<Resource> {
		self.access
			.artifact_creation_resource(task, owner)
			.await
			.map_err(Into::into)
	}
	async fn snapshot(&mut self, workspace: Uuid) -> Result<Value> {
		self.access
			.workspace_snapshot(workspace)
			.await
			.map(|value| json!(value))
			.map_err(Into::into)
	}
	async fn record(&mut self, workspace: Uuid, kind: &str, id: Uuid) -> Result<Value> {
		self.access
			.workspace_record(workspace, kind, id)
			.await
			.map_err(Into::into)
	}
	async fn children(&mut self, workspace: Uuid, parent: Uuid) -> Result<Value> {
		self.access
			.workspace_children(workspace, parent)
			.await
			.map(|value| json!(value))
			.map_err(Into::into)
	}
	async fn created_by_grant(&mut self, grant: Uuid, child: Uuid) -> Result<bool> {
		let result: NativeResult<bool> = async {
let created_by_grant: bool = { let query_bind_1 = grant; let query_bind_2 = child; sqlx::query_scalar(&Query::select()
						.expr(SimpleExpr::CustomWithExpr("(EXISTS (SELECT 1 FROM authorization_remote_outputs WHERE grant_id=? AND resource_kind='task' AND resource_id=?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()]))
						.to_string(PostgresQueryBuilder))
				.fetch_one(&mut **self.access.tx)
				.await? };
            Ok(created_by_grant)
        }.await;
		result.map_err(Into::into)
	}
	async fn apply(&mut self, context: WriteContext<'_>, effect: Effect<'_>) -> Result<Applied> {
		let WriteContext {
			task,
			owner,
			key,
			admission,
		} = context;
		let result: NativeResult<Applied> = async {
			let store = &self.runtime.store;
			let (value, output_id) = match effect {
				Effect::Claim { revision, agent } => (
					json!(
						store
							.claim_in(&mut self.access.tx, task, revision, owner, agent)
							.await?
					),
					None,
				),
				Effect::Transition {
					revision,
					next,
					through,
				} => {
					let value = if let Some(seq) = through {
						store
							.transition_remote_terminal_in(
								&mut self.access.tx,
								task.id,
								revision,
								owner,
								next,
								admission,
								crate::store::TerminalRunMessageInputs::Through(seq),
							)
							.await?
					} else {
						store
							.transition_in(&mut self.access.tx, task.id, revision, owner, next)
							.await?
					};
					(json!(value), None)
				}
				Effect::Artifact {
					artifact,
					complete_through,
				} => {
					if let Some(through) = complete_through {
						let value = store
							.complete_in(
								&mut self.access.tx,
								task.id,
								owner,
								key,
								artifact,
								None,
								Some((admission, through)),
							)
							.await?;
						let id: Uuid = {
							let query_bind_1 = key;
							sqlx::query_scalar(
								&Query::select()
									.column(Alias::new("id"))
									.from(Alias::new("artifacts"))
									.and_where(SimpleExpr::CustomWithExpr(
										"(idempotency_key=?)".to_owned(),
										vec![Expr::value(query_bind_1.to_owned()).into()],
									))
									.to_string(PostgresQueryBuilder),
							)
							.fetch_one(&mut **self.access.tx)
							.await?
						};
						(json!(value), Some(id))
					} else {
						let value = store
							.publish_artifact_in(
								&mut self.access.tx,
								task.id,
								owner,
								key,
								artifact,
								None,
							)
							.await?;
						let id = value.id;
						(json!(value), Some(id))
					}
				}
				Effect::CreateTask { task: new } => {
					let value = store
						.create_task_in(
							&mut self.access.tx,
							task.workspace_id,
							new,
							owner,
							Some(key),
						)
						.await?;
					let id = value.id;
					(json!(value), Some(id))
				}
				Effect::Delegate { child, agent } => (
					json!(
						crate::authorization::execution::delegate_in(
							self.runtime,
							self.access,
							child,
							agent
						)
						.await?
					),
					None,
				),
				Effect::Message { content, through } => {
					let value = if let Some(seq) = through {
						store
							.run_message_output_in(
								&mut self.access.tx,
								crate::store::FencedRunMessageOutput {
									workspace: task.workspace_id,
									task_id: task.id,
									run_id: admission,
									included_input_seq: seq,
									sender: owner,
									content,
									key,
								},
							)
							.await?
					} else {
						store
							.message_in(
								&mut self.access.tx,
								task.workspace_id,
								owner,
								content,
								Some(key),
							)
							.await?
					};
					let id = value.id;
					(json!(value), Some(id))
				}
				Effect::ReserveInput { node, key, content } => {
					store
						.reserve_remote_run_message_in(
							&mut self.access.tx,
							task.id,
							admission,
							node,
							key,
							content,
						)
						.await?;
					(json!({"reserved":true}), None)
				}
				Effect::CommitInput { key, content, seq } => {
					store
						.commit_remote_run_message_in(
							&mut self.access.tx,
							task.id,
							admission,
							key,
							content,
							Some(seq),
						)
						.await?;
					(json!({"committed":true}), None)
				}
				Effect::DeliverInput { node, key, content } => {
					let full_key = format!("{node}:{}:{key}", task.id);
					let sender = format!("human@{node}");
					let value = store
						.run_message_delivery_in(
							&mut self.access.tx,
							crate::store::RunMessageDelivery {
								workspace: task.workspace_id,
								task_id: task.id,
								run_id: admission,
								sender: &sender,
								content,
								input_key: key,
								message_key: &full_key,
							},
						)
						.await?;
					(json!(value), None)
				}
				Effect::ReleaseInputs { node, keys } => {
					store
						.release_remote_run_message_in(
							&mut self.access.tx,
							task.id,
							admission,
							node,
							keys,
						)
						.await?;
					(json!({"acknowledged":true}), None)
				}
				Effect::Event { kind, data } => {
					store
						.event(&mut self.access.tx, Some(task.workspace_id), kind, data)
						.await?;
					(json!({"recorded":true}), None)
				}
			};
			Ok(Applied { value, output_id })
		}
		.await;
		result.map_err(Into::into)
	}
	async fn record_output(
		&mut self,
		grant: Uuid,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<()> {
		let result: NativeResult<()> = async {
			sqlx::query(
				&Query::insert()
					.into_table(Alias::new("authorization_remote_outputs"))
					.columns(
						["grant_id", "workspace_id", "resource_kind", "resource_id"]
							.map(Alias::new),
					)
					.from_subquery(((1..=4).map(|i| Expr::cust(format!("${i}")))).fold(
						reinhardt::query::Query::select(),
						|mut select, expr| {
							select.expr(expr);
							select
						},
					))
					.on_conflict(
						OnConflict::columns([
							"grant_id",
							"workspace_id",
							"resource_kind",
							"resource_id",
						])
						.do_nothing()
						.to_owned(),
					)
					.to_string(PostgresQueryBuilder),
			)
			.bind(grant)
			.bind(workspace)
			.bind(kind)
			.bind(id)
			.execute(&mut **self.access.tx)
			.await?;
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn history(
		&mut self,
		workspace: Uuid,
		task: Uuid,
		node: &str,
		offset: usize,
	) -> Result<Vec<Message>> {
		let result: NativeResult<Vec<Message>> = async {
			let rows: Vec<crate::domain::Message> = {
				let query_bind_1 = workspace;
				let query_bind_2 = format!("{node}:{}:%", task);
				let query_bind_3 = format!("human@{node}");
				aidash_server::database::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("messages"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(workspace_id=? AND idempotency_key LIKE ? AND sender=?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
								Expr::value(query_bind_3.to_owned()).into(),
							],
						))
						.order_by(Alias::new("created_at"), reinhardt::query::Order::Asc)
						.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
						.limit(4)
						.offset(offset as u64)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **self.access.tx)
				.await?
			};
			Ok(rows)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn persist_replay(
		&mut self,
		grant: Uuid,
		task: Uuid,
		key: &str,
		digest: &str,
		result: &Value,
	) -> Result<()> {
		let result: NativeResult<()> = async {
			let current: i64 = {
				let query_bind_1 = task;
				sqlx::query_scalar(
					&Query::select()
						.column(Alias::new("revision"))
						.from(Alias::new("tasks"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **self.access.tx)
				.await?
			};
			{
				let query_bind_1 = grant;
				let query_bind_2 = current;
				sqlx::query(
					&Query::update()
						.table(Alias::new("authorization_remote_execution"))
						.value_expr(
							Alias::new("task_revision"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						)
						.and_where(SimpleExpr::CustomWithExpr(
							"(grant_id=?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **self.access.tx)
				.await?
			};
			sqlx::query(
				&Query::insert()
					.into_table(Alias::new("authorization_remote_commands"))
					.columns(["grant_id", "request_key", "digest", "result"].map(Alias::new))
					.from_subquery(
						Query::select()
							.expr(Expr::cust("$1"))
							.expr(Expr::cust("$2"))
							.expr(Expr::cust("$3"))
							.expr(Expr::cust("$4"))
							.to_owned(),
					)
					.to_string(PostgresQueryBuilder),
			)
			.bind(grant)
			.bind(key)
			.bind(digest)
			.bind(result)
			.execute(&mut **self.access.tx)
			.await?;
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn live(&mut self, grant: Uuid) -> Result<bool> {
		crate::authorization::remote::live(self.access, grant)
			.await
			.map_err(Into::into)
	}
}
