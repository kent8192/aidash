use super::{Manifest, Mutation};
use crate::{
	Error, Result,
	domain::{Artifact, Run, Task},
	store::Store,
};
use serde_json::json;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

/// SQL-only mutations. Preparation uses a savepoint to validate this exact
/// implementation; no provider, tool or peer effect can run inside it.
pub(super) async fn apply(
	store: &Store,
	tx: &mut Transaction<'_, Postgres>,
	manifest: &Manifest,
) -> Result<()> {
	for mutation in &manifest.local(&store.node_id)?.mutations {
		match mutation {
			Mutation::RegistryRegister { entry } => {
				if crate::registry::register_in(tx, entry, &store.node_id).await? {
					store
						.event(
							tx,
							None,
							"registry.registered",
							json!({"id":entry.id,"version":entry.version}),
						)
						.await?;
				}
			}
			Mutation::WorkspaceState {
				workspace_id,
				expected_revision,
				state,
			} => {
				store
					.update_state_in(tx, *workspace_id, *expected_revision, state.clone())
					.await?;
			}
			Mutation::CompleteTask {
				task_id,
				expected_revision,
				artifact,
			} => {
				let task: Task = sqlx::query_as(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::SimpleExpr::from(
							sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
						))
						.from(sea_orm::sea_query::Alias::new("tasks"))
						.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
						.lock(sea_orm::sea_query::LockType::Update)
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(task_id)
				.fetch_optional(&mut **tx)
				.await?
				.ok_or_else(|| Error::Conflict("task unavailable".into()))?;
				if task.revision != *expected_revision
					|| task.status != "RUNNING"
					|| task.owner.is_none()
				{
					return Err(Error::Conflict(
						"task must be running at its expected revision".into(),
					));
				}
				let children:bool=sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::Expr::cust("EXISTS(SELECT 1 FROM tasks WHERE parent_id = $1 AND NOT status IN ('COMPLETED', 'ABANDONED'))")).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(task_id).fetch_one(&mut **tx).await?;
				if children {
					return Err(Error::Conflict("task still has unfinished children".into()));
				}
				let delegated: Option<String> = sqlx::query_scalar(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::SimpleExpr::from(
							sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new(
								"node_id",
							)),
						))
						.from(sea_orm::sea_query::Alias::new("delegations"))
						.and_where(sea_orm::sea_query::Expr::cust("task_id = $1"))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(task_id)
				.fetch_optional(&mut **tx)
				.await?;
				if let Some(node) = delegated {
					let paired = manifest
						.local(&node)?
						.mutations
						.iter()
						.any(|m| matches!(m,Mutation::FinishRun{task_id:id,..} if id==task_id));
					if !paired {
						return Err(Error::Invalid("delegated completion requires its participant's execution finalization".into()));
					}
				}
				let key = format!("atomic:{}:task:{}", manifest.id, task.id);
				let created: Artifact = sqlx::query_as(
					&sea_orm::sea_query::Query::insert()
						.into_table(sea_orm::sea_query::Alias::new("artifacts"))
						.columns([
							sea_orm::sea_query::Alias::new("id"),
							sea_orm::sea_query::Alias::new("workspace_id"),
							sea_orm::sea_query::Alias::new("task_id"),
							sea_orm::sea_query::Alias::new("kind"),
							sea_orm::sea_query::Alias::new("name"),
							sea_orm::sea_query::Alias::new("content"),
							sea_orm::sea_query::Alias::new("created_by"),
							sea_orm::sea_query::Alias::new("idempotency_key"),
						])
						.values_panic([
							sea_orm::sea_query::Expr::cust("$1"),
							sea_orm::sea_query::Expr::cust("$2"),
							sea_orm::sea_query::Expr::cust("$3"),
							sea_orm::sea_query::Expr::cust("$4"),
							sea_orm::sea_query::Expr::cust("$5"),
							sea_orm::sea_query::Expr::cust("$6"),
							sea_orm::sea_query::Expr::cust("$7"),
							sea_orm::sea_query::Expr::cust("$8"),
						])
						.returning_all()
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(Uuid::new_v4())
				.bind(task.workspace_id)
				.bind(task.id)
				.bind(&artifact.kind)
				.bind(&artifact.name)
				.bind(&artifact.content)
				.bind(&task.owner)
				.bind(&key)
				.fetch_one(&mut **tx)
				.await?;
				let saved: Task = sqlx::query_as(
					&sea_orm::sea_query::Query::update()
						.table(sea_orm::sea_query::Alias::new("tasks"))
						.value(
							sea_orm::sea_query::Alias::new("status"),
							sea_orm::sea_query::Expr::cust("'COMPLETED'"),
						)
						.value(
							sea_orm::sea_query::Alias::new("completion_key"),
							sea_orm::sea_query::Expr::cust("$2"),
						)
						.value(
							sea_orm::sea_query::Alias::new("revision"),
							sea_orm::sea_query::Expr::cust("revision + 1"),
						)
						.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
						.returning_all()
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(task.id)
				.bind(key)
				.fetch_one(&mut **tx)
				.await?;
				let source: Option<Uuid> = sqlx::query_scalar(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::SimpleExpr::from(
							sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("run_id")),
						))
						.from(sea_orm::sea_query::Alias::new("authorization_execution"))
						.and_where(sea_orm::sea_query::Expr::cust("task_id = $1"))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(task.id)
				.fetch_optional(&mut **tx)
				.await?;
				store
					.record_output_in(tx, source, task.workspace_id, "artifact", created.id)
					.await?;
				store
					.event(
						tx,
						Some(task.workspace_id),
						"task.completed",
						json!({"task":saved,"artifact":created}),
					)
					.await?;
			}
			Mutation::FinishRun {
				run_id,
				task_id,
				expected_revision,
			} => {
				let run: Run = sqlx::query_as(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::SimpleExpr::from(
							sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
						))
						.from(sea_orm::sea_query::Alias::new("runs"))
						.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
						.lock(sea_orm::sea_query::LockType::Update)
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(run_id)
				.fetch_optional(&mut **tx)
				.await?
				.ok_or_else(|| Error::Conflict("run unavailable".into()))?;
				let leased: bool = sqlx::query_scalar(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::Expr::cust(
							"lease_until > CLOCK_TIMESTAMP()",
						))
						.from(sea_orm::sea_query::Alias::new("runs"))
						.and_where(sea_orm::sea_query::Expr::cust(
							"id = $1 AND lease_until IS NOT NULL",
						))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(run_id)
				.fetch_optional(&mut **tx)
				.await?
				.unwrap_or(false);
				if run.task_id != *task_id
					|| run.revision != *expected_revision
					|| run.phase != "TOOL_CALL"
					|| run.control == "CANCELLED"
					|| leased
				{
					return Err(Error::Conflict(
						"run must be quiescent at its expected tool-call revision".into(),
					));
				}
				let response: crate::provider::ModelResponse =
					serde_json::from_value(run.pending["response"].clone())
						.map_err(|_| Error::Conflict("run has no final model response".into()))?;
				if !response.tool_calls.is_empty() {
					return Err(Error::Conflict("run still has pending tools".into()));
				}
				if !manifest
					.local(&run.home_node)?
					.mutations
					.iter()
					.any(|m| matches!(m,Mutation::CompleteTask{task_id:id,..} if id==task_id))
				{
					return Err(Error::Invalid(
						"execution finalization requires the home task's atomic completion".into(),
					));
				}
				sqlx::query(
					&sea_orm::sea_query::Query::update()
						.table(sea_orm::sea_query::Alias::new("runs"))
						.value(
							sea_orm::sea_query::Alias::new("phase"),
							sea_orm::sea_query::Expr::cust("'COMPLETED'"),
						)
						.value(
							sea_orm::sea_query::Alias::new("pending"),
							sea_orm::sea_query::Expr::cust("CAST('{}' AS JSONB)"),
						)
						.value(
							sea_orm::sea_query::Alias::new("error"),
							sea_orm::sea_query::Expr::cust("NULL"),
						)
						.value(
							sea_orm::sea_query::Alias::new("revision"),
							sea_orm::sea_query::Expr::cust("revision + 1"),
						)
						.value(
							sea_orm::sea_query::Alias::new("lease_owner"),
							sea_orm::sea_query::Expr::cust("NULL"),
						)
						.value(
							sea_orm::sea_query::Alias::new("lease_until"),
							sea_orm::sea_query::Expr::cust("NULL"),
						)
						.value(
							sea_orm::sea_query::Alias::new("updated_at"),
							sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP"),
						)
						.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(run_id)
				.execute(&mut **tx)
				.await?;
				store.event(tx,(run.home_node==store.node_id).then_some(run.workspace_id),"run.completed",json!({"run_id":run.id,"task_id":run.task_id,"workspace_id":run.workspace_id,"agent_id":run.agent_id,"phase":"COMPLETED","step":run.step,"error":null,"context_usage":run.context.get("usage")})).await?;
			}
		}
	}
	Ok(())
}
