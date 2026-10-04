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
	config.core_capabilities = input.core_capabilities;
	config.skill_attachments = input.skill_attachments;
	config.skill_roots = input.skill_roots;
	config.reference_attachments = input.reference_attachments;
	aidash_domain::capabilities::skills::validate_config(&config)?;
	aidash_domain::capabilities::references::validate_config(&config)?;
	if config.reference_attachments.len() > scope.limits().files {
		return Err(Error::Invalid("REFERENCE_SET_LIMIT".into()));
	}
	let mut extracted = 0;
	for binding in &config.reference_attachments {
		let record = scope.reference(binding.reference_id).await?;
		if record.state != "ready" || record.data["original"]["digest"] != binding.digest {
			return Err(Error::Conflict("REFERENCE_NOT_READY_OR_CHANGED".into()));
		}
		if record.data["original"]["size"].as_u64().unwrap_or(u64::MAX) > scope.limits().bytes {
			return Err(Error::Invalid("REFERENCE_SET_LIMIT".into()));
		}
		extracted += record.data["extraction"]["size"].as_u64().unwrap_or(0);
	}
	if extracted > scope.limits().text_bytes as u64 {
		return Err(Error::Invalid("REFERENCE_SET_LIMIT".into()));
	}
	if entry.installation.is_some() {
		return Err(Error::Invalid(
			"use installation.configure for installed definitions".into(),
		));
	}
	entry.version = input.new_version;
	entry.config = serde_json::to_value(config)?;
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
		// Preserve historical text-only attachments exactly, without fabricating originals.
		scope
			.preserve_documents(&id, &entry.version, &input.source_version)
			.await?;
		scope.event("capability.agent_configured",json!({"agent_id":id,"source_version":input.source_version,"version":entry.version,"actor":scope.principal()})).await?;
	}
	let result = json!({"entry":entry,"catalog_approval_required":true});
	scope.cache(input.idempotency_key, &digest, &result).await?;
	Ok(result)
}
