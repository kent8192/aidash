//! Subject-scoped immutable capability editing does not grant catalog permission.
use crate::{Error, Result, ports::capabilities::configuration::ConfigurationScope};
use aidash_domain::{
	capabilities::configuration::Configure,
	registry::{AgentConfig, EntityRef},
};
use serde_json::{Value, json};
pub async fn configure(
	scope: &mut dyn ConfigurationScope,
	id: String,
	input: Configure,
) -> Result<Value> {
	let digest =
		aidash_domain::registry::rules::digest(&json!(["capability_configuration", id, input]));
	let mut entry = scope
		.entry(
			&EntityRef {
				id: id.clone(),
				version: input.source_version.clone(),
			},
			"agent.configure",
		)
		.await?;
	if entry.kind != "agent" {
		return Err(Error::Forbidden);
	}
	if let Some(cached) = scope.cached(input.idempotency_key, &digest).await? {
		return Ok(cached);
	}
	if input.new_version == input.source_version {
		return Err(Error::Conflict("AGENT_VERSION_IMMUTABLE".into()));
	}
	let mut config: AgentConfig = serde_json::from_value(entry.config.clone())?;
	config.bindings = input.bindings;
	config.remove_default = input.remove_default;
	config.definition().validate()?;
	if entry.installation.is_some() {
		return Err(Error::Invalid(
			"use installation.configure for installed definitions".into(),
		));
	}
	entry.binding_normalization = None;
	entry.version = input.new_version;
	entry.config = serde_json::to_value(config)?;
	let snapshot = scope.bindings(&entry).await?;
	let admitted = AgentConfig::from_snapshot(&snapshot)?;
	validate_references(scope, &admitted).await?;
	let inserted = scope.register(&entry).await?;
	if inserted {
		scope
			.provenance(
				&EntityRef {
					id: id.clone(),
					version: input.source_version.clone(),
				},
				&entry,
			)
			.await?;
		scope.event("capability.agent_configured",json!({"agent_id":id,"source_version":input.source_version,"version":entry.version,"actor":scope.principal()})).await?;
	}
	let result = json!({"entry":entry,"catalog_approval_required":true});
	scope.cache(input.idempotency_key, &digest, &result).await?;
	Ok(result)
}

/// Attachment declarations do not bypass current reference authority or aggregate bounds.
async fn validate_references(
	scope: &mut dyn ConfigurationScope,
	config: &AgentConfig,
) -> Result<()> {
	let limits = scope.limits();
	if config.reference_attachments.len() > limits.files {
		return Err(Error::Invalid("REFERENCE_SET_LIMIT".into()));
	}
	let mut seen = std::collections::BTreeSet::new();
	let mut text_bytes = 0u64;
	for attachment in &config.reference_attachments {
		if !seen.insert(attachment.reference_id) {
			return Err(Error::Invalid("DUPLICATE_REFERENCE".into()));
		}
		let reference = scope.reference(attachment.reference_id).await?;
		if reference.state != "ready" {
			return Err(Error::Conflict("REFERENCE_NOT_READY".into()));
		}
		let original: aidash_domain::capabilities::operations::MountedFile =
			serde_json::from_value(reference.data["original"].clone())?;
		if original.digest != attachment.digest {
			return Err(Error::Conflict("REFERENCE_CHANGED".into()));
		}
		if original.size > limits.bytes {
			return Err(Error::Invalid("REFERENCE_SET_LIMIT".into()));
		}
		let extracted: Option<aidash_domain::capabilities::operations::MountedFile> =
			serde_json::from_value(reference.data["extraction"].clone())?;
		text_bytes = text_bytes
			.checked_add(extracted.as_ref().map_or(0, |f| f.size))
			.ok_or_else(|| Error::Invalid("REFERENCE_SET_LIMIT".into()))?;
	}
	if text_bytes > limits.text_bytes as u64 {
		return Err(Error::Invalid("REFERENCE_SET_LIMIT".into()));
	}
	Ok(())
}
