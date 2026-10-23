//! Current built-in compaction default, pinned before definition hashing (ADR 0084).
use super::*;

pub const COMPACTION_DECIDER_ID: &str = "aidash.compaction-decider";
pub const COMPACTION_DECIDER_VERSION: &str = "1.0.0";
/// Writable `remove_default` key; it is independent of the selected exact version.
pub const COMPACTION_DEFAULT: &str = "compaction";

pub fn compaction_reference(node: &str) -> QualifiedRef {
	QualifiedRef {
		registry_deployment: node.into(),
		id: COMPACTION_DECIDER_ID.into(),
		version: COMPACTION_DECIDER_VERSION.into(),
	}
}

pub(super) fn add_compaction(
	config: &AgentBindings,
	node: &str,
	replacement: Option<&QualifiedRef>,
	bindings: &mut Vec<NormalizedBinding>,
) -> Result<()> {
	if let Some(replacement) = replacement {
		if replacement.registry_deployment != node
			|| !config.bindings.iter().any(|binding| {
				binding.kind == BindingKind::Decider && &binding.target == replacement
			}) {
			return Err(Error::Invalid(
				"compaction replacement requires an explicit execution-node Decider Binding".into(),
			));
		}
		return Ok(());
	}
	let target = compaction_reference(node);
	if config
		.remove_default
		.iter()
		.any(|name| name == COMPACTION_DEFAULT)
		|| bindings.iter().any(|bound| bound.binding.target == target)
	{
		return Ok(());
	}
	bindings.push(NormalizedBinding {
		binding: Binding {
			kind: BindingKind::Decider,
			target,
			alias: None,
			narrow: Narrowing::default(),
			members: vec![],
			exposure: None,
		},
		origin: BindingOrigin::Default,
	});
	Ok(())
}

/// Compaction authorization is checked when needed, before every dispatch/apply;
/// an oversized context pauses with the decision layer's typed reason.
pub fn is_compaction_decider(entry: &Entry) -> bool {
	entry.kind == "decider"
		&& serde_json::from_value::<crate::decision::DeciderConfig>(entry.config.clone())
			.is_ok_and(|config| config.hook == crate::decision::Hook::Compaction)
}

/// Templates do not confer an implicit compaction dependency on generated chains.
pub fn is_implicit_compaction(snapshot: &BindingSnapshot, identity: &QualifiedRef) -> bool {
	snapshot.bindings.iter().any(|binding| {
		&binding.identity == identity
			&& binding.origin == BindingOrigin::Default
			&& is_compaction_decider(&binding.definition)
	})
}
