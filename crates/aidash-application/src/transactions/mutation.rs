//! Apply local manifest mutations and their events in the caller's original scope.
use crate::{Error, Result, ports::transactions::mutation::MutationScope};
use aidash_domain::transactions::{Manifest, Mutation, Participant, mutation as rules};
use serde_json::json;

/// The participant for a Node address.
fn participant<'a>(manifest: &'a Manifest, address: &str) -> Result<&'a Participant> {
	manifest.local(address).map_err(|_| Error::Forbidden)
}

/// Preparation and commit invoke this same workflow; storage alone owns the
/// rollback-only savepoint and the visibility gate around the caller's transaction.
pub async fn apply(scope: &mut dyn MutationScope, node: &str, manifest: &Manifest) -> Result<()> {
	for mutation in &participant(manifest, node)?.mutations {
		match mutation {
			Mutation::WorkspaceState {
				workspace_id,
				expected_revision,
				state,
			} => {
				let workspace = scope
					.replace_workspace(*workspace_id, *expected_revision, state.clone())
					.await?;
				scope
					.append_event(Some(*workspace_id), "workspace.updated", json!(workspace))
					.await?;
			}
			Mutation::CompleteTask {
				task_id,
				expected_revision,
				artifact,
			} => {
				let task = scope.lock_task(*task_id).await?;
				rules::validate_task(&task, *expected_revision)?;
				if scope.unfinished_children(*task_id).await? {
					return Err(Error::Conflict("task still has unfinished children".into()));
				}
				if let Some(executor) = scope.delegated_node(*task_id).await? {
					rules::validate_delegated_completion(
						participant(manifest, &executor)?,
						*task_id,
					)?;
				}
				let key = format!("atomic:{}:task:{}", manifest.id, task.id);
				let (saved, created) = scope.complete_task(&task, artifact, &key).await?;
				if let Some(run) = scope.source_run(task.id).await? {
					scope
						.record_output(run, task.workspace_id, "artifact", created.id)
						.await?;
				}
				scope
					.append_event(
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
				let (raw, leased) = scope.lock_run(*run_id).await?;
				rules::validate_run(&raw.metadata, leased, *task_id, *expected_revision)?;
				// Check revision, phase, cancellation and lease before decoding state.
				let decoded = raw.decode()?;
				if !decoded.state.tool()?.response.tool_calls.is_empty() {
					return Err(Error::Conflict("run still has pending tools".into()));
				}
				let run = &raw.metadata;
				rules::validate_home_completion(participant(manifest, &run.home_node)?, *task_id)?;
				scope.complete_run(*run_id).await?;
				scope.append_event(run.is_local(aidash_domain::nodes::deployment_of(node)).then_some(run.workspace_id), "run.completed", json!({"run_id":run.id,"task_id":run.task_id,"workspace_id":run.workspace_id,"agent_id":run.agent_id,"phase":"COMPLETED","step":run.step,"error":null,"context_usage":raw.context.get("usage")})).await?;
			}
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests;
