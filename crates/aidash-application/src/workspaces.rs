//! Authorized conversation admission, initial task creation, and delegation.
use crate::{Result, ports::workspaces::ConversationScope};
use aidash_domain::{
	NewTask,
	registry::EntityRef,
	workspaces::{ConversationResponse, conversation_agent, validate_conversation_target},
};
use serde_json::json;

pub struct ConversationRequest<'a> {
	pub title: &'a str,
	pub goal: &'a str,
	pub target: &'a EntityRef,
	pub target_kind: &'a str,
}

pub async fn conversation(
	scope: &mut dyn ConversationScope,
	input: ConversationRequest<'_>,
) -> Result<ConversationResponse> {
	let workspace = scope.create_workspace(input.title, input.goal).await?;
	scope.workspace_context(workspace.id).await?;
	scope.require_workspace("workspace.read").await?;
	let entry = scope.registry_entry(input.target, "registry.read").await?;
	validate_conversation_target(&entry, input.target_kind)?;
	if input.target_kind == "cluster" {
		scope
			.registry_entry(input.target, "cluster.execute")
			.await?;
	}
	let agent = conversation_agent(&entry, input.target)?;
	let conversation = scope
		.create_conversation(workspace.id, input.target, input.target_kind)
		.await?;
	scope
		.require_conversation(&conversation, "conversation.create")
		.await?;
	scope
		.require_conversation(&conversation, "conversation.read")
		.await?;
	scope.conversation_event(&conversation).await?;
	scope.require_workspace("message.create").await?;
	scope.message(workspace.id, input.goal).await?;
	scope.require_workspace("task.create").await?;
	let task = scope
		.create_task(
			workspace.id,
			&NewTask {
				title: input.title.into(),
				description: input.goal.into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
		)
		.await?;
	scope.require_task(&task, "task.read").await?;
	scope.bind_task(task.id, conversation.id).await?;
	let delegation = scope.delegate(task.id, &agent).await?;
	let task = scope.task(task.id).await?;
	Ok(ConversationResponse {
		conversation,
		workspace,
		task,
		delegation,
	})
}

#[cfg(test)]
mod tests;

pub mod channels;

pub mod mutations;
