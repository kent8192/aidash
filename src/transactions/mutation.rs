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
                crate::registry::register_in(tx, entry).await?;
                store
                    .event(
                        tx,
                        None,
                        "registry.registered",
                        json!({"id":entry.id,"version":entry.version}),
                    )
                    .await?;
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
                let task: Task = sqlx::query_as("SELECT * FROM tasks WHERE id=$1 FOR UPDATE")
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
                let children:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tasks WHERE parent_id=$1 AND status NOT IN ('COMPLETED','ABANDONED'))").bind(task_id).fetch_one(&mut **tx).await?;
                if children {
                    return Err(Error::Conflict("task still has unfinished children".into()));
                }
                let delegated: Option<String> =
                    sqlx::query_scalar("SELECT node_id FROM delegations WHERE task_id=$1")
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
                let created:Artifact=sqlx::query_as("INSERT INTO artifacts(id,workspace_id,task_id,kind,name,content,created_by,idempotency_key) VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING *")
                    .bind(Uuid::new_v4()).bind(task.workspace_id).bind(task.id).bind(&artifact.kind).bind(&artifact.name).bind(&artifact.content).bind(&task.owner).bind(&key).fetch_one(&mut **tx).await?;
                let saved:Task=sqlx::query_as("UPDATE tasks SET status='COMPLETED',completion_key=$2,revision=revision+1 WHERE id=$1 RETURNING *").bind(task.id).bind(key).fetch_one(&mut **tx).await?;
                let source: Option<Uuid> = sqlx::query_scalar(
                    "SELECT run_id FROM authorization_execution WHERE task_id=$1",
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
                let run: Run = sqlx::query_as("SELECT * FROM runs WHERE id=$1 FOR UPDATE")
                    .bind(run_id)
                    .fetch_optional(&mut **tx)
                    .await?
                    .ok_or_else(|| Error::Conflict("run unavailable".into()))?;
                let leased:bool=sqlx::query_scalar("SELECT lease_until>clock_timestamp() FROM runs WHERE id=$1 AND lease_until IS NOT NULL").bind(run_id).fetch_optional(&mut **tx).await?.unwrap_or(false);
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
                sqlx::query("UPDATE runs SET phase='COMPLETED',pending='{}'::jsonb,error=NULL,revision=revision+1,lease_owner=NULL,lease_until=NULL,updated_at=now() WHERE id=$1").bind(run_id).execute(&mut **tx).await?;
                store.event(tx,(run.home_node==store.node_id).then_some(run.workspace_id),"run.completed",json!({"run_id":run.id,"task_id":run.task_id,"workspace_id":run.workspace_id,"agent_id":run.agent_id,"phase":"COMPLETED","step":run.step,"error":null,"context_usage":run.context.get("usage")})).await?;
            }
        }
    }
    Ok(())
}
