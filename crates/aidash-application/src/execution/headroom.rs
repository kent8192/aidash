//! Corrections share the registered model window with task and pinned Skill context.
use crate::{Result, ports::execution::headroom::Definitions};
use aidash_domain::{
	RunMetadata, context::MIN_CONTEXT_RESERVE, model::ModelConfig, registry::AgentConfig,
};
use serde_json::{Value, json};

pub async fn request(scope: &dyn Definitions, run: &RunMetadata) -> Result<usize> {
	let snapshot = scope.snapshot(run).await?;
	let agent = AgentConfig::from_snapshot(&snapshot)?;
	let mut documents = vec![];
	for binding in &snapshot.bindings {
		if binding.definition.kind == "source"
			&& binding.definition.config.get("schema_version").is_some()
		{
			let source: aidash_domain::registry::bindings::sources::NativeContext =
				serde_json::from_value(binding.definition.config.clone())?;
			if matches!(
				source.source,
				aidash_domain::registry::bindings::sources::NativeSource::PrivateReferences { .. }
			) {
				let contents = scope.documents(&binding.definition).await?;
				documents.extend(
					contents
						.as_array()
						.ok_or_else(|| {
							crate::Error::Invalid(
								"private Source contents must be a document array".into(),
							)
						})?
						.iter()
						.cloned(),
				);
			}
		}
	}
	let private_context = if documents.is_empty() {
		Value::Null
	} else {
		json!({"reference_documents":documents})
	};
	let available = scope
		.validation()
		.bound_prompt_headroom(&snapshot, &private_context)?
		.saturating_sub(MIN_CONTEXT_RESERVE);

	// Deferred registration already reserves the full exposure budgets, and a
	// deferred Run never marks pinned Skills loaded.
	if !agent.core_capabilities.skills || agent.exposure_policy().is_deferred() {
		return Ok(available);
	}
	Ok(available.saturating_sub(scope.pinned_headroom(run.id).await?))
}

pub async fn message_limit(scope: &dyn Definitions, run: &RunMetadata) -> Result<usize> {
	// Retain most of the window for task, workspace observation and tool history.
	Ok((request(scope, run).await? / 4).min(16_384))
}

pub async fn media_routes(scope: &dyn Definitions, run: &RunMetadata) -> Result<Vec<Vec<String>>> {
	if run.home_node != scope.node() {
		return Ok(vec![]);
	}
	let snapshot = scope.snapshot(run).await?;
	let agent = AgentConfig::from_snapshot(&snapshot)?;
	let entry = snapshot
		.definitions
		.iter()
		.find(|d| {
			d.identity.local() == agent.model
				&& d.identity.registry_node == snapshot.agent.registry_node
		})
		.ok_or(crate::Error::Forbidden)?
		.definition
		.clone();
	let model: ModelConfig = serde_json::from_value(entry.config)?;
	Ok(model.current_media_input_routes())
}

#[cfg(test)]
mod tests;
