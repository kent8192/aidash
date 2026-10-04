//! Worker effects retain their producer, delegated creator and admitted credential.
use crate::{Error, Result, ports::authorization::worker_tasks::WorkerTaskScope};
use aidash_domain::{
	NewTask, RunMetadata, Task,
	federation::Delegation,
	generation::requests::Assignment,
	identity::execution::{CreatedTaskOrigin, TaskOrigin},
	qualified_agent,
	registry::EntityRef,
};
use uuid::Uuid;
pub async fn assign(
	scope: &mut dyn WorkerTaskScope,
	run: &RunMetadata,
	task: Uuid,
	policy: &str,
	reason: &str,
) -> Result<Assignment> {
	if scope.task_workspace(task).await? != Some(run.workspace_id) {
		return Err(Error::Forbidden);
	}
	let assignment = scope.assign(task, policy, reason).await?;
	scope.record_output(run, "task", task).await?;
	if let Assignment::Generated { generation } = &assignment {
		scope
			.record_output(run, "generation", generation.id)
			.await?;
	}
	Ok(assignment)
}
pub async fn create_task(
	scope: &mut dyn WorkerTaskScope,
	run: &RunMetadata,
	key: &str,
	input: &NewTask,
) -> Result<Task> {
	let workspace = scope.workspace(run.workspace_id).await?;
	scope.require(&workspace, "task.create").await?;
	scope.related_tasks(run.workspace_id, input).await?;
	let creator = scope.subjects().last().ok_or(Error::Forbidden)?.clone();
	let task = scope
		.create_task(run.workspace_id, input, &creator, key)
		.await?;
	let resource = scope.task_resource(&task).await?;
	scope.require(&resource, "task.read").await?;
	scope.record_output(run, "task", task.id).await?;
	scope.insert_origin(task.id, run.id).await?;
	let actual = scope.created_origin(task.id).await?;
	let identity = scope.identity();
	let expected = CreatedTaskOrigin {
		source_run_id: run.id,
		authority: TaskOrigin {
			tenant: identity.tenant,
			root_subject: identity.subject,
			subject_chain: scope.subjects().to_vec(),
		},
	};
	if actual != expected {
		return Err(Error::Conflict(
			"task already has a different origin".into(),
		));
	}
	Ok(task)
}
pub async fn delegate(
	scope: &mut dyn WorkerTaskScope,
	run: &RunMetadata,
	task: Uuid,
	node: &str,
	agent: &EntityRef,
) -> Result<Delegation> {
	if node != scope.node_id() {
		return Err(Error::Forbidden);
	}
	if scope.task_workspace(task).await? != Some(run.workspace_id) {
		return Err(Error::Forbidden);
	}
	let record = scope.task_read(task).await?;
	let resource = scope.task_resource(&record).await?;
	scope.require(&resource, "task.delegate").await?;
	if let Some(existing) = scope.execution_grant(task).await? {
		let mut expected = scope.subjects().to_vec();
		expected.push(qualified_agent(node, &agent.id, &agent.version));
		if existing.subject_chain != expected
			|| existing.credential_id != scope.identity().credential_id
		{
			return Err(Error::Conflict(
				"task already has a different authority".into(),
			));
		}
		return Ok(Delegation {
			task_id: task,
			node_id: node.into(),
			agent_id: agent.id.clone(),
			agent_version: agent.version.clone(),
			delivered: true,
		});
	}
	scope.delegate(task, agent).await
}
#[cfg(test)]
mod tests;
