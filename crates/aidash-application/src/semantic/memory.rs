//! Managed agent memory and delivery reauthorization are shared business workflows.
use crate::{
	Result,
	ports::semantic::memory::{SemanticMemoryReadSession, SemanticMemoryWriteSession},
};
use aidash_domain::{
	Run, qualified_agent,
	semantic::{Source, indexing::content_digest, mutations::Put},
};
use serde_json::{Value, json};
use uuid::Uuid;

pub async fn remember(
	scope: &mut dyn SemanticMemoryWriteSession,
	run: &Run,
	data: &Value,
) -> Result<()> {
	if scope.configured(run.workspace_id).await? {
		let agent = qualified_agent(&run.home_node, &run.agent_id, &run.agent_version);
		let key = format!("agent-memory:{}", content_digest(&agent));
		// Lock the index before reading the current revision, including worker writes.
		scope.index(run.workspace_id, true).await?;
		let revision = scope.revision(run.workspace_id, &key).await?;
		let entry = super::mutations::put(
			scope,
			run.workspace_id,
			Put {
				key,
				expected_revision: revision.unwrap_or(0),
				source: Source::Memory {
					text: serde_json::to_string(data)?,
				},
				agent: Some(agent),
				metadata: json!({"origin":"agent_memory"}),
			},
		)
		.await?;
		scope.bind_memory(&entry, run).await?;
	}
	scope.persist_memory(run, data).await
}

pub async fn reads_visible(scope: &mut dyn SemanticMemoryReadSession, run: Uuid) -> Result<bool> {
	for (id, revision) in scope.dependencies(run).await? {
		let Some(entry) = scope.entry(id).await? else {
			return Ok(false);
		};
		if entry.deleted
			|| entry.revision != revision
			|| !scope.permits(&entry, "semantic.read").await?
		{
			return Ok(false);
		}
		let Some(text) = scope
			.source(entry.workspace_id, &serde_json::from_value(entry.source)?)
			.await?
		else {
			return Ok(false);
		};
		if scope.point_digest(entry.point_id).await?.as_deref() != Some(&content_digest(&text)) {
			return Ok(false);
		}
	}
	Ok(true)
}

#[cfg(test)]
mod tests;
