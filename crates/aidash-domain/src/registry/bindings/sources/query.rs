//! One combined semantic retrieval cannot silently inherit another Source's query policy.
use super::*;
use crate::context::source::QueryPolicy;

/// Every effective semantic contributor, including the primary native memory
/// provider, pins the same policy. An omitted declaration means task-inputs@1.
/// This does not validate the full snapshot, because admission calls it while
/// validating that snapshot's closure.
pub fn semantic_query_policy(snapshot: &super::super::BindingSnapshot) -> Result<QueryPolicy> {
	let mut selected = None;
	for binding in snapshot.bindings.iter().filter(|binding| {
		binding.excluded_reason.is_none()
			&& matches!(binding.definition.kind.as_str(), "memory" | "source")
	}) {
		let query = if binding.definition.config.get("schema_version").is_some() {
			let native: NativeContext = serde_json::from_value(binding.definition.config.clone())?;
			if !matches!(
				native.source,
				NativeSource::ConversationMemory { .. }
					| NativeSource::SemanticMemory {}
					| NativeSource::WorkspaceRetrieval { .. }
			) {
				continue;
			}
			native
				.context_source
				.map(|declaration| declaration.query)
				.unwrap_or_default()
		} else if binding.definition.kind == "memory" {
			let provider: crate::memory::ProviderConfig =
				serde_json::from_value(binding.definition.config.clone())?;
			provider
				.context_source
				.map(|declaration| declaration.query)
				.unwrap_or_default()
		} else {
			let source: crate::memory::SourceConfig =
				serde_json::from_value(binding.definition.config.clone())?;
			source
				.context_source
				.map(|declaration| declaration.query)
				.unwrap_or_default()
		};
		if selected.is_some_and(|prior| prior != query) {
			return Err(Error::Invalid("semantic Sources require one effective query policy, including implicit task-inputs@1".into()));
		}
		selected = Some(query);
	}
	Ok(selected.unwrap_or_default())
}
