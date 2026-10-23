//! Built-in workspace observation is a required bounded Source, like workspace_read.
use super::*;
pub const WORKSPACE_OBSERVATION_ID: &str = "aidash.workspace-observation";
pub const WORKSPACE_OBSERVATION_VERSION: &str = "1.0.0";
pub fn workspace_observation_reference(node: &str) -> QualifiedRef {
	QualifiedRef {
		registry_deployment: node.into(),
		id: WORKSPACE_OBSERVATION_ID.into(),
		version: WORKSPACE_OBSERVATION_VERSION.into(),
	}
}
pub(super) fn add_workspace_observation(
	node: &str,
	bindings: &mut Vec<NormalizedBinding>,
) -> Result<()> {
	let target = workspace_observation_reference(node);
	if let Some(binding) = bindings.iter_mut().find(|b| b.binding.target == target) {
		if binding.binding.kind != BindingKind::Source {
			return Err(Error::Invalid(
				"workspace observation must be bound as a Source".into(),
			));
		}
		binding.origin = BindingOrigin::Required;
	} else {
		bindings.push(NormalizedBinding {
			binding: Binding {
				kind: BindingKind::Source,
				target,
				alias: None,
				narrow: Narrowing::default(),
				members: vec![],
				exposure: None,
			},
			origin: BindingOrigin::Required,
		});
	}
	Ok(())
}
pub fn workspace_observation() -> Result<Entry> {
	Ok(Entry {
		binding_normalization: None,
		installation: None,
		id: WORKSPACE_OBSERVATION_ID.into(),
		version: WORKSPACE_OBSERVATION_VERSION.into(),
		kind: "source".into(),
		name: [("en".into(), "Workspace observation".into())].into(),
		description: [(
			"en".into(),
			"Bounded authorized workspace state at the current inference boundary".into(),
		)]
		.into(),
		capabilities: vec![],
		tags: vec!["system".into()],
		languages: vec![],
		skills: vec![],
		schema: serde_json::json!({}),
		config: serde_json::to_value(sources::NativeContext {
			schema_version: BINDING_SCHEMA,
			source: sources::NativeSource::WorkspaceObservation {},
			context_source: None,
		})?,
	})
}
