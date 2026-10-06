//! Managed agent memory and delivery reauthorization are shared business workflows.
use crate::{Result, ports::semantic::memory::SemanticMemoryReadSession};
use aidash_domain::semantic::indexing::content_digest;
use uuid::Uuid;

pub async fn reads_visible(scope: &mut dyn SemanticMemoryReadSession, run: Uuid) -> Result<bool> {
	if !scope.native_reads_visible(run).await? {
		return Ok(false);
	}
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
