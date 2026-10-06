//! Creation and mutations share authorization with worker and conversation callers.
use crate::{Result, ports::workspaces::mutations::WorkspaceMutations};
use aidash_domain::{NewTask, Task, Workspace};
use serde_json::{Value, json};
use uuid::Uuid;
pub async fn create(
	scope: &mut dyn WorkspaceMutations,
	id: Uuid,
	title: &str,
	goal: &str,
) -> Result<Workspace> {
	let resource = scope.resource(
		"workspace",
		id,
		json!({"owner":scope.identity().1,"workspace_id":id}),
	);
	scope.require(&resource, "workspace.create").await?;
	let workspace = scope.insert_workspace(id, title, goal).await?;
	scope.record_owner(id).await?;
	Ok(workspace)
}
pub async fn update(
	scope: &mut dyn WorkspaceMutations,
	id: Uuid,
	revision: i64,
	state: Value,
) -> Result<Workspace> {
	scope.require_workspace(id, "workspace.read").await?;
	scope.require_workspace(id, "workspace.update").await?;
	scope.update_state(id, revision, state).await
}
pub async fn create_task(
	scope: &mut dyn WorkspaceMutations,
	id: Uuid,
	input: &NewTask,
	key: Option<&str>,
) -> Result<Task> {
	scope.require_workspace(id, "workspace.read").await?;
	scope.require_workspace(id, "task.create").await?;
	let key = key.map(|key| {
		format!(
			"subject:{}",
			aidash_domain::registry::rules::digest(&json!([
				scope.identity().0,
				scope.identity().1,
				id,
				key
			]))
		)
	});
	scope.related_tasks(id, input).await?;
	let subject = scope.identity().1.to_owned();
	let task = scope
		.insert_task(id, input, &subject, key.as_deref())
		.await?;
	let resource = scope.task_resource(&task).await?;
	scope.require(&resource, "task.read").await?;
	Ok(task)
}
pub async fn message(
	scope: &mut dyn WorkspaceMutations,
	id: Uuid,
	content: &str,
	nonce: Uuid,
) -> Result<()> {
	let key = format!(
		"workspace-subject:{}:{}:{id}:{nonce}",
		scope.identity().0,
		scope.identity().1
	);
	scope.require_workspace(id, "message.create").await?;
	let sender = scope.identity().1.to_owned();
	scope.insert_message(id, &sender, content, &key).await
}
#[cfg(test)]
mod tests;
