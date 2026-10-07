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
	let scoped = scope.scoped();
	if !scoped && !scope.operator_visible(entry.workspace_id).await? {
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
	if scoped && action == "semantic.read" && !entry.deleted {
		let decoded = serde_json::from_value(entry.source.clone())?;
		if !matches!(decoded, Source::Memory { .. })
			&& source(scope, entry.workspace_id, &decoded).await?.is_none()
		{
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
	if !scope.scoped() && !scope.operator_visible(workspace).await? {
		return Ok(None);
	}
	match source {
		Source::Unit { id } => scope.unit(*id, workspace).await,
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
