//! Stored ownership, output provenance, and immutable event membership govern disclosure.
use crate::{
	Error, Result,
	ports::authorization::visibility::{
		conclude_chain,
		resources::{ResourceAccessScope, ResourceEventScope, ResourceVisibilityScope},
	},
};
use aidash_domain::{Conversation, Event, HumanRequest, Message, NewTask, Task, policy::Resource};
use serde_json::json;
use uuid::Uuid;
pub async fn task_resource(
	scope: &mut dyn ResourceVisibilityScope,
	task: &Task,
) -> Result<Resource> {
	let thread = scope.thread(task.thread_id).await?;
	let mut a = thread.attributes;
	a["created_by"] = json!(task.created_by);
	a["task_id"] = json!(task.id);
	Ok(scope.resource("task", &task.id.to_string(), a))
}
pub async fn message_resource(
	scope: &mut dyn ResourceVisibilityScope,
	message: &Message,
) -> Result<Resource> {
	let thread = scope.thread(message.thread_id).await?;
	let mut a = thread.attributes;
	a["created_by"] = json!(message.sender);
	a["sender"] = json!(message.sender);
	Ok(scope.resource("message", &message.id.to_string(), a))
}
pub async fn human_resource(
	scope: &mut dyn ResourceVisibilityScope,
	request: &HumanRequest,
) -> Result<Resource> {
	let thread = scope.thread(request.thread_id).await?;
	let mut a = thread.attributes;
	a["kind"] = json!(request.kind);
	a["run_id"] = json!(request.run_id);
	Ok(scope.resource("human_request", &request.id.to_string(), a))
}
pub async fn conversation_resource(
	scope: &mut dyn ResourceVisibilityScope,
	conversation: &Conversation,
) -> Result<Resource> {
	let thread = scope.thread(conversation.thread_id).await?;
	let mut a = thread.attributes;
	a["created_by"] = json!(conversation.created_by);
	a["target"] = json!(conversation.target);
	a["target_kind"] = json!(conversation.target_kind);
	Ok(scope.resource("conversation", &conversation.id.to_string(), a))
}
pub async fn task_visible(scope: &mut dyn ResourceVisibilityScope, task: &Task) -> Result<bool> {
	let resource = task_resource(scope, task).await?;
	Ok(scope.decide(&resource, "task.read").await?
		&& scope
			.output_visible(task.thread_id, "task", task.id)
			.await?)
}
pub async fn task_summary_visible(
	scope: &mut dyn ResourceVisibilityScope,
	thread: &Resource,
	thread_id: Uuid,
	task_id: Uuid,
	created_by: &str,
) -> Result<bool> {
	let mut a = thread.attributes.clone();
	a["created_by"] = json!(created_by);
	a["task_id"] = json!(task_id);
	let resource = scope.resource("task", &task_id.to_string(), a);
	Ok(scope.decide(&resource, "task.read").await?
		&& scope.output_visible(thread_id, "task", task_id).await?)
}
pub async fn message_visible(
	scope: &mut dyn ResourceVisibilityScope,
	message: &Message,
) -> Result<bool> {
	let resource = message_resource(scope, message).await?;
	Ok(scope.decide(&resource, "message.read").await?
		&& scope
			.output_visible(message.thread_id, "message", message.id)
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
	thread: Uuid,
	run: Uuid,
) -> Result<bool> {
	// Only immutable request decisions are cached. Re-read membership on every call.
	for request in scope.humans(thread, run).await? {
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
/// Which check refused a Task that exists: its `task.read` policy, or the Read
/// dependencies of the Runs that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRefusal {
	Policy,
	Outputs,
}
/// [`task_read`] that says which check refused. A missing Task is still
/// [`Error::Forbidden`].
pub async fn task_read_explained(
	scope: &mut dyn ResourceAccessScope,
	id: Uuid,
) -> Result<std::result::Result<Task, TaskRefusal>> {
	let task = scope.task_any(id).await?.ok_or(Error::Forbidden)?;
	let resource = task_resource(scope, &task).await?;
	if !scope.decide(&resource, "task.read").await? {
		return Ok(Err(TaskRefusal::Policy));
	}
	if !scope
		.output_visible(task.thread_id, "task", task.id)
		.await?
	{
		return Ok(Err(TaskRefusal::Outputs));
	}
	Ok(Ok(task))
}
pub async fn related_tasks(
	scope: &mut dyn ResourceAccessScope,
	thread: Uuid,
	input: &NewTask,
) -> Result<()> {
	for id in input.dependencies.iter().chain(input.parent_id.iter()) {
		if task_read(scope, *id).await?.thread_id != thread {
			return Err(Error::Forbidden);
		}
	}
	Ok(())
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
		let Some(task) = scope.event_task(task_id, event.thread_id).await? else {
			return Ok(Some(false));
		};
		let task_allowed = task_visible(scope, &task).await?;
		let held = scope.take_row_silence();
		let mut message_allowed = true;
		if let Some(message) = id(&event.data["message_id"]) {
			let rows = scope.messages_id(message, event.thread_id).await?;
			message_allowed = if let Some(message) = rows.first() {
				message_visible(scope, message).await?
			} else {
				false
			};
		}
		if !task_allowed {
			conclude_chain(scope, held, message_allowed);
		}
		return Ok(Some(task_allowed && message_allowed));
	}
	// No reply thread is opened since #1008; an event recorded before then is
	// visible exactly when its root message is.
	if event.kind == "message.created" || event.kind == "message.thread_opened" {
		if event.kind == "message.thread_opened" && id(&event.data["id"]).is_none() {
			return Ok(Some(false));
		}
		let messages = if let Some(message) = id(&event.data["id"]) {
			scope.messages_id(message, event.thread_id).await?
		} else {
			scope
				.messages_legacy(
					event.thread_id,
					event.data["sender"].as_str(),
					event.data["content"].as_str(),
				)
				.await?
		};
		if messages.is_empty() {
			return Ok(Some(false));
		}
		// Legacy events without an ID must authorize every matching immutable row.
		// A row that stops at a silent Node does not end the chain: the others are
		// judged too, and a refusal among them is not counted (#770).
		let mut held: Option<String> = None;
		for message in messages {
			if message_visible(scope, &message).await? {
				continue;
			}
			match (held.is_some(), scope.take_row_silence()) {
				(false, None) => return Ok(Some(false)),
				(false, node) => held = node,
				(true, None) => return Ok(Some(false)),
				(true, Some(_)) => {}
			}
		}
		if held.is_some() {
			conclude_chain(scope, held, true);
			return Ok(Some(false));
		}
		return Ok(Some(true));
	}
	Ok(None)
}
#[cfg(test)]
mod tests;
