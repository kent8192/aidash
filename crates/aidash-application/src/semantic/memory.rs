//! Managed agent memory and delivery reauthorization are shared business workflows.
use crate::{Result, ports::semantic::memory::SemanticMemoryReadSession};
use aidash_domain::semantic::{Failure, Source, indexing::content_digest};
use uuid::Uuid;

/// The message a Thread Attachment image was posted in, whose text its match carries.
fn attachment_message(source: &Source) -> Option<Source> {
	match source {
		Source::AttachmentImage { message_id, .. } => Some(Source::Message { id: *message_id }),
		_ => None,
	}
}

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
		let source: Source = serde_json::from_value(entry.source.clone())?;
		if source.is_image() {
			// An image match showed a pointer and, for a Thread Attachment, the
			// text of its message: the image and that message must still be
			// readable. Its point holds no text to compare; an edited
			// description changes the entry's revision instead.
			if !scope.image_visible(entry.workspace_id, &source).await? {
				return Ok(false);
			}
			if let Some(message) = attachment_message(&source)
				&& scope.source(entry.workspace_id, &message).await?.is_none()
			{
				return Ok(false);
			}
			continue;
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
	pending: &mut Vec<Uuid>,
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
		let source: Source = serde_json::from_value(entry.source.clone())?;
		if scope
			.source_invalidated(entry.workspace_id, &source)
			.await?
		{
			return Ok(Some(Failure::Invalidated));
		}
		if source.is_image() {
			if !scope.permits(&entry, "semantic.read").await?
				|| !scope.image_visible(entry.workspace_id, &source).await?
			{
				return Ok(Some(Failure::Authority));
			}
			// The message an attachment match quoted joins the classified walk,
			// as a message match does.
			if let Some(message) = attachment_message(&source)
				&& scope
					.classified_output_source(entry.workspace_id, &message, pending)
					.await?
					.is_none()
			{
				return Ok(Some(Failure::Authority));
			}
			continue;
		}
		let text = if matches!(source, Source::Artifact { .. } | Source::Message { .. }) {
			scope
				.classified_output_source(entry.workspace_id, &source, pending)
				.await?
		} else {
			if !scope.permits(&entry, "semantic.read").await? {
				return Ok(Some(Failure::Authority));
			}
			scope.source(entry.workspace_id, &source).await?
		};
		let Some(text) = text else {
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
