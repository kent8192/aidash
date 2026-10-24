//! Stored ownership, output provenance, and immutable event membership govern disclosure.
use crate::{
	Error, Result,
	ports::authorization::visibility::resources::{
		ResourceAccessScope, ResourceEventScope, ResourceVisibilityScope,
	},
};
use aidash_domain::{
	Artifact, Conversation, Event, HumanRequest, Message, NewTask, Task, policy::Resource,
};
use serde_json::json;
use uuid::Uuid;
pub async fn task_resource(
	scope: &mut dyn ResourceVisibilityScope,
	task: &Task,
) -> Result<Resource> {
	let workspace = scope.workspace(task.workspace_id).await?;
	let mut a = workspace.attributes;
	a["created_by"] = json!(task.created_by);
	a["task_id"] = json!(task.id);
	Ok(scope.resource("task", &task.id.to_string(), a))
}
pub async fn artifact_resource(
	scope: &mut dyn ResourceVisibilityScope,
	artifact: &Artifact,
) -> Result<Resource> {
	let workspace = scope.workspace(artifact.workspace_id).await?;
	let mut a = workspace.attributes;
	a["created_by"] = json!(artifact.created_by);
	a["task_id"] = json!(artifact.task_id);
	a["kind"] = json!(artifact.kind);
	Ok(scope.resource("artifact", &artifact.id.to_string(), a))
}
pub async fn message_resource(
	scope: &mut dyn ResourceVisibilityScope,
	message: &Message,
) -> Result<Resource> {
	let workspace = scope.workspace(message.workspace_id).await?;
	let mut a = workspace.attributes;
	a["created_by"] = json!(message.sender);
	a["sender"] = json!(message.sender);
	Ok(scope.resource("message", &message.id.to_string(), a))
}
pub async fn human_resource(
	scope: &mut dyn ResourceVisibilityScope,
	request: &HumanRequest,
) -> Result<Resource> {
	let workspace = scope.workspace(request.workspace_id).await?;
	let mut a = workspace.attributes;
	a["kind"] = json!(request.kind);
	a["run_id"] = json!(request.run_id);
	Ok(scope.resource("human_request", &request.id.to_string(), a))
}
pub async fn conversation_resource(
	scope: &mut dyn ResourceVisibilityScope,
	conversation: &Conversation,
) -> Result<Resource> {
	let workspace = scope.workspace(conversation.workspace_id).await?;
	let mut a = workspace.attributes;
	a["created_by"] = json!(conversation.created_by);
	a["target"] = json!(conversation.target);
	a["target_kind"] = json!(conversation.target_kind);
	Ok(scope.resource("conversation", &conversation.id.to_string(), a))
}
pub async fn task_visible(scope: &mut dyn ResourceVisibilityScope, task: &Task) -> Result<bool> {
	let resource = task_resource(scope, task).await?;
	Ok(scope.decide(&resource, "task.read").await?
		&& scope
			.output_visible(task.workspace_id, "task", task.id)
			.await?)
}
pub async fn task_summary_visible(
	scope: &mut dyn ResourceVisibilityScope,
	workspace: &Resource,
	workspace_id: Uuid,
	task_id: Uuid,
	created_by: &str,
) -> Result<bool> {
	let mut a = workspace.attributes.clone();
	a["created_by"] = json!(created_by);
	a["task_id"] = json!(task_id);
	let resource = scope.resource("task", &task_id.to_string(), a);
	Ok(scope.decide(&resource, "task.read").await?
		&& scope.output_visible(workspace_id, "task", task_id).await?)
}
pub async fn artifact_visible(
	scope: &mut dyn ResourceVisibilityScope,
	artifact: &Artifact,
) -> Result<bool> {
	let resource = artifact_resource(scope, artifact).await?;
	if !scope.decide(&resource, "artifact.read").await? {
		return Ok(false);
	}
	match scope.artifact_task(artifact).await? {
		Some(task) if task_visible(scope, &task).await? => {}
		_ => return Ok(false),
	}
	scope
		.output_visible(artifact.workspace_id, "artifact", artifact.id)
		.await
}
pub async fn message_visible(
	scope: &mut dyn ResourceVisibilityScope,
	message: &Message,
) -> Result<bool> {
	let resource = message_resource(scope, message).await?;
	Ok(scope.decide(&resource, "message.read").await?
		&& scope
			.output_visible(message.workspace_id, "message", message.id)
			.await?)
}
pub async fn human_visible(
	scope: &mut dyn ResourceVisibilityScope,
	request: &HumanRequest,
) -> Result<bool> {
	if let Some(allowed) = scope.cached_human(request.id) {
		return Ok(allowed);
	}
	let resource = human_resource(scope, request).await?;
	let allowed = scope.decide(&resource, "human.read").await?;
	scope.remember_human(request.id, allowed);
	Ok(allowed)
}
pub async fn human_reads(
	scope: &mut dyn ResourceVisibilityScope,
	workspace: Uuid,
	run: Uuid,
) -> Result<bool> {
	// Only immutable request decisions are cached. Re-read membership on every call.
	for request in scope.humans(workspace, run).await? {
		if !human_visible(scope, &request).await? {
			return Ok(false);
		}
	}
	Ok(true)
}
pub async fn task_read(scope: &mut dyn ResourceAccessScope, id: Uuid) -> Result<Task> {
	let task = scope.task_any(id).await?.ok_or(Error::Forbidden)?;
	if !task_visible(scope, &task).await? {
		return Err(Error::Forbidden);
	}
	Ok(task)
}
pub async fn related_tasks(
	scope: &mut dyn ResourceAccessScope,
	workspace: Uuid,
	input: &NewTask,
) -> Result<()> {
	for id in input.dependencies.iter().chain(input.parent_id.iter()) {
		if task_read(scope, *id).await?.workspace_id != workspace {
			return Err(Error::Forbidden);
		}
	}
	Ok(())
}
pub async fn artifact_creation_resource(
	scope: &mut dyn ResourceAccessScope,
	task: Uuid,
	creator: &str,
) -> Result<Resource> {
	let task = task_read(scope, task).await?;
	let mut resource = task_resource(scope, &task).await?;
	resource.kind = "artifact".into();
	resource.attributes["created_by"] = json!(creator);
	Ok(resource)
}
pub async fn artifact_id_visible(
	scope: &mut dyn ResourceEventScope,
	id: Uuid,
	workspace: Option<Uuid>,
) -> Result<bool> {
	match scope.artifact(id, workspace).await? {
		Some(artifact) => artifact_visible(scope, &artifact).await,
		None => Ok(false),
	}
}
pub async fn event_visible(
	scope: &mut dyn ResourceEventScope,
	event: &Event,
) -> Result<Option<bool>> {
	let id = |v: &serde_json::Value| v.as_str().and_then(|s| s.parse::<Uuid>().ok());
	if event.kind.starts_with("task.") {
		let Some(task_id) = id(&event.data["task"]["id"])
			.or_else(|| id(&event.data["task_id"]))
			.or_else(|| id(&event.data["id"]))
		else {
			return Ok(Some(false));
		};
		let Some(task) = scope.event_task(task_id, event.workspace_id).await? else {
			return Ok(Some(false));
		};
		if !task_visible(scope, &task).await? {
			return Ok(Some(false));
		}
		if let Some(artifact) = id(&event.data["artifact"]["id"]) {
			return Ok(Some(
				artifact_id_visible(scope, artifact, event.workspace_id).await?,
			));
		}
		return Ok(Some(true));
	}
	if event.kind.starts_with("artifact.") {
		let Some(artifact) = id(&event.data["id"]) else {
			return Ok(Some(false));
		};
		return Ok(Some(
			artifact_id_visible(scope, artifact, event.workspace_id).await?,
		));
	}
	if event.kind == "message.created" || event.kind == "message.thread_opened" {
		if event.kind == "message.thread_opened" && id(&event.data["id"]).is_none() {
			return Ok(Some(false));
		}
		let messages = if let Some(message) = id(&event.data["id"]) {
			scope.messages_id(message, event.workspace_id).await?
		} else {
			scope
				.messages_legacy(
					event.workspace_id,
					event.data["sender"].as_str(),
					event.data["content"].as_str(),
				)
				.await?
		};
		if messages.is_empty() {
			return Ok(Some(false));
		}
		// Legacy events without an ID must authorize every matching immutable row.
		for message in messages {
			if !message_visible(scope, &message).await? {
				return Ok(Some(false));
			}
		}
		return Ok(Some(true));
	}
	Ok(None)
}
#[cfg(test)]
mod tests;
