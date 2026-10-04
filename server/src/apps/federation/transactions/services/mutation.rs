//! Apply validated mutations without provider, tool, or peer effects.
use super::{Manifest, Mutation};
use crate::apps::execution::models::{
	Run, event_records,
	states::{RunControl, RunPhase},
};
use crate::apps::federation::remote::models::Delegation;
use crate::apps::identity::models::{AuthorizationExecution, AuthorizationRunOutput};
use crate::apps::registry::services::admission;
use crate::apps::workspaces::models::{Task, Workspace, states::TaskStatus};
use crate::{Error, Result, store::Store};
use reinhardt::db::backends::TransactionExecutor;
use serde_json::json;

/// Preparation invokes this same implementation under a rollback-only savepoint.
pub(super) async fn apply(
	store: &Store,
	tx: &mut dyn TransactionExecutor,
	manifest: &Manifest,
) -> Result<()> {
	for mutation in &manifest.local(&store.node_id)?.mutations {
		match mutation {
			Mutation::RegistryRegister { entry } => {
				if admission::register(tx, entry, &store.node_id).await? {
					event_records::append(
						tx,
						&store.node_id,
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
				let workspace =
					Workspace::replace_state(tx, *workspace_id, *expected_revision, state.clone())
						.await?;
				event_records::append(
					tx,
					&store.node_id,
					Some(*workspace_id),
					"workspace.updated",
					json!(workspace),
				)
				.await?;
			}
			Mutation::CompleteTask {
				task_id,
				expected_revision,
				artifact,
			} => {
				let task = Task::lock(tx, *task_id).await?;
				if task.revision != *expected_revision
					|| task.status != TaskStatus::Running
					|| task.owner.is_none()
				{
					return Err(Error::Conflict(
						"task must be running at its expected revision".into(),
					));
				}
				if Task::unfinished_children(tx, *task_id).await? {
					return Err(Error::Conflict("task still has unfinished children".into()));
				}
				if let Some(node) = Delegation::node_for_task(tx, *task_id).await? {
					let paired = manifest.local(&node)?.mutations.iter().any(
						|m| matches!(m, Mutation::FinishRun { task_id: id, .. } if id == task_id),
					);
					if !paired {
						return Err(Error::Invalid("delegated completion requires its participant's execution finalization".into()));
					}
				}
				let key = format!("atomic:{}:task:{}", manifest.id, task.id);
				let (saved, created) = Task::complete(tx, &task, artifact, &key).await?;
				if let Some(run) = AuthorizationExecution::source_run_for_task(tx, task.id).await? {
					AuthorizationRunOutput::record(
						tx,
						run,
						task.workspace_id,
						"artifact",
						created.id,
					)
					.await?;
				}
				event_records::append(
					tx,
					&store.node_id,
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
				let (run, leased) = Run::lock_with_lease(tx, *run_id).await?;
				if run.task_id != *task_id
					|| run.revision != *expected_revision
					|| run.phase != RunPhase::ToolCall
					|| run.control == RunControl::Cancelled
					|| leased
				{
					return Err(Error::Conflict(
						"run must be quiescent at its expected tool-call revision".into(),
					));
				}
				let decoded: crate::domain::Run = run.clone().try_into()?;
				let response = &decoded.state.tool()?.response;
				if !response.tool_calls.is_empty() {
					return Err(Error::Conflict("run still has pending tools".into()));
				}
				if !manifest.local(&run.home_node)?.mutations.iter().any(
					|m| matches!(m, Mutation::CompleteTask { task_id: id, .. } if id == task_id),
				) {
					return Err(Error::Invalid(
						"execution finalization requires the home task's atomic completion".into(),
					));
				}
				Run::complete(tx, *run_id).await?;
				event_records::append(tx, &store.node_id,
					(run.home_node == store.node_id).then_some(run.workspace_id), "run.completed",
					json!({"run_id":run.id,"task_id":run.task_id,"workspace_id":run.workspace_id,"agent_id":run.agent_id,"phase":"COMPLETED","step":run.step,"error":null,"context_usage":run.context.get("usage")})).await?;
			}
		}
	}
	Ok(())
}
