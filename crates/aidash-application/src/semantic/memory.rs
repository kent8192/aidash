//! Managed agent memory and delivery reauthorization are shared business workflows.
use crate::{Result, ports::semantic::memory::SemanticMemoryReadSession};
use aidash_domain::semantic::{Failure, indexing::content_digest};
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

pub async fn reads_invalidity(
	scope: &mut dyn SemanticMemoryReadSession,
	run: Uuid,
) -> Result<Option<Failure>> {
	if let Some(failure) = scope.native_invalidity(run).await? {
		return Ok(Some(failure));
	}
	for (id, revision) in scope.dependencies(run).await? {
		let Some(entry) = scope.entry(id).await? else {
			return Ok(Some(Failure::Invalidated));
		};
		if !scope.entry_authorized(&entry).await? {
			return Ok(Some(Failure::Authority));
		}
		if entry.deleted || entry.revision != revision {
			return Ok(Some(Failure::Invalidated));
		}
		let source = serde_json::from_value(entry.source.clone())?;
		if scope
			.source_invalidated(entry.workspace_id, &source)
			.await?
		{
			return Ok(Some(Failure::Invalidated));
		}
		if !scope.permits(&entry, "semantic.read").await? {
			return Ok(Some(Failure::Authority));
		}
		let Some(text) = scope.source(entry.workspace_id, &source).await? else {
			return Ok(Some(Failure::Authority));
		};
		if scope.point_digest(entry.point_id).await?.as_deref() != Some(&content_digest(&text)) {
			return Ok(Some(Failure::Invalidated));
		}
	}
	Ok(None)
}

#[cfg(test)]
mod tests;
