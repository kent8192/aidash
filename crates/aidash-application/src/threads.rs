//! A person's request opens a Thread with its Root Task: authorized
//! admission, the Root Task, and its delegation (#1008, ADR 0140).
use crate::{Error, Result, ports::threads::ConversationScope};
use aidash_domain::{
	NewTask, nodes::NodeName, registry::EntityRef, semantic::indexing::IndexingSpec,
	threads::ConversationResponse,
};
use serde_json::json;

pub struct ConversationRequest<'a> {
	pub title: &'a str,
	/// The person's request: the first message and the Root Task's description.
	pub request: &'a str,
	pub target: &'a EntityRef,
	pub target_kind: &'a str,
	/// The Node the Thread goes to; the Tenant's default Node when absent.
	pub node: Option<&'a NodeName>,
	pub scratch_space_id: Option<uuid::Uuid>,
}

pub async fn conversation(
	scope: &mut dyn ConversationScope,
	input: ConversationRequest<'_>,
) -> Result<ConversationResponse> {
	let target_node = match input.target_kind {
		"agent" => None,
		"node" => Some(NodeName::parse(&input.target.id)?),
		_ => {
			return Err(Error::Invalid(
				"Thread target must be an Agent or Node".into(),
			));
		}
	};
	if let (Some(target), Some(node)) = (&target_node, input.node)
		&& target != node
	{
		return Err(Error::Invalid(
			"Thread Node differs from its target Node".into(),
		));
	}
	let thread = scope
		.create_thread(
			input.title,
			target_node.as_ref().or(input.node),
			input.scratch_space_id,
		)
		.await?;
	scope.thread_context(thread.id).await?;
	scope.require_thread("thread.read").await?;
	let agent = if target_node.is_none() {
		let entry = scope.registry_entry(input.target, "registry.read").await?;
		if entry.kind != "agent" {
			return Err(Error::Invalid("Thread target must be an Agent".into()));
		}
		Some(input.target)
	} else {
		None
	};
	let scratch_space_id = scope
		.select_scratch_space(&thread, input.title, input.scratch_space_id)
		.await?;
	let conversation = scope
		.create_conversation(thread.id, scratch_space_id, input.target, input.target_kind)
		.await?;
	scope
		.require_conversation(&conversation, "conversation.create")
		.await?;
	scope
		.require_conversation(&conversation, "conversation.read")
		.await?;
	scope.conversation_event(&conversation).await?;
	scope.require_thread("message.create").await?;
	scope.message(thread.id, input.request).await?;
	scope.require_thread("task.create").await?;
	let task = scope
		.create_task(
			thread.id,
			&NewTask {
				title: input.title.into(),
				description: input.request.into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
		)
		.await?;
	scope.require_task(&task, "task.read").await?;
	// The Thread exists only from this request on, so nothing else could
	// give it the index the agent's memory bank requires (#647).
	if let Some(agent) = agent
		&& let Some(memory) = scope.memory_embedding(agent).await?
	{
		scope.require_thread("semantic.index.manage").await?;
		scope
			.create_memory_index(
				&conversation,
				agent,
				&memory.provider,
				&IndexingSpec::for_memory(memory.embedding),
			)
			.await?;
	}
	let delegation = if let Some(agent) = agent {
		Some(scope.delegate(task.id, agent).await?)
	} else {
		None
	};
	let task = scope.task(task.id).await?;
	Ok(ConversationResponse {
		conversation,
		thread,
		task,
		delegation,
	})
}

#[cfg(test)]
mod tests;

pub mod channels;

pub mod mutations;
