//! Current source and managed-memory policy applies to reads, replays and history.
use crate::{Result, ports::semantic::visibility::SemanticDisclosureScope};
use aidash_domain::semantic::{Source, mutations::Entry};
use serde_json::json;
use uuid::Uuid;
pub async fn permits(
	scope: &mut dyn SemanticDisclosureScope,
	entry: &Entry,
	action: &str,
) -> Result<bool> {
	if !permits_resource(scope, entry, action).await? {
		return Ok(false);
	}
	if scope.scoped() && action == "semantic.read" && !entry.deleted {
		let decoded = serde_json::from_value(entry.source.clone())?;
		// An attachment image is readable only with the message it was posted in.
		let decoded = match decoded {
			Source::AttachmentImage { message_id, .. } => Source::Message { id: message_id },
			other => other,
		};
		if !matches!(decoded, Source::Memory { .. } | Source::WorkingImage { .. })
			&& source(scope, entry.workspace_id, &decoded).await?.is_none()
		{
			return Ok(false);
		}
	}
	Ok(true)
}

/// Check the semantic entry's own authority without resolving its source.
pub async fn permits_resource(
	scope: &mut dyn SemanticDisclosureScope,
	entry: &Entry,
	action: &str,
) -> Result<bool> {
	let scoped = scope.scoped();
	if !scoped
		&& !scope
			.deployment_administrator_visible(entry.workspace_id)
			.await?
	{
		return Ok(false);
	}
	if scoped {
		let workspace = scope.workspace_resource(entry.workspace_id).await?;
		let mut attributes = workspace.attributes;
		attributes["created_by"] = json!(entry.created_by);
		attributes["agent"] = json!(entry.agent);
		attributes["metadata"] = entry.metadata.clone();
		let resource = scope.resource("semantic", &entry.id.to_string(), attributes.clone());
		if !scope.decide(&resource, action).await? {
			return Ok(false);
		}
	}
	Ok(true)
}
pub async fn source(
	scope: &mut dyn SemanticDisclosureScope,
	workspace: Uuid,
	source: &Source,
) -> Result<Option<String>> {
	if !scope.scoped() && !scope.deployment_administrator_visible(workspace).await? {
		return Ok(None);
	}
	match source {
		Source::Unit { id } => scope.unit(*id, workspace).await,
		// Images are disclosed only as pointers, through their own read paths.
		Source::WorkingImage { .. } | Source::AttachmentImage { .. } => Ok(None),
		Source::Memory { text } => Ok(Some(text.clone())),
		Source::Artifact { id } => {
			let Some(row) = scope.artifact(*id, workspace).await? else {
				return Ok(None);
			};
			if scope.scoped() && !scope.artifact_visible(&row).await? {
				return Ok(None);
			}
			Ok(Some(serde_json::to_string(&row.content)?))
		}
		Source::Message { id } => {
			let Some(row) = scope.message(*id, workspace).await? else {
				return Ok(None);
			};
			if scope.scoped() && !scope.message_visible(&row).await? {
				return Ok(None);
			}
			Ok(Some(row.content))
		}
	}
}
#[cfg(test)]
mod tests;
