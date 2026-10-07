//! Rebuild the Agent's capability closure from pinned definitions at durable boundaries.
use super::*;
use crate::tool::providers::{ToolDescriptor, reserved_aliases, validate_restrictions};

impl BindingSnapshot {
	pub(super) fn validate_closure(
		&self,
		config: &AgentBindings,
		definitions: &BTreeMap<&QualifiedRef, &ResolvedDefinition>,
	) -> Result<()> {
		let mut pending = config
			.normalize(&self.agent.registry_node)?
			.into_iter()
			.map(|binding| (binding, BTreeSet::new(), None::<(String, String)>))
			.collect::<Vec<_>>();
		let mut expected = BTreeMap::<QualifiedRef, NormalizedBinding>::new();
		let mut source_skill_support = false;
		while let Some((mut normalized, mut ancestry, lifecycle)) = pending.pop() {
			let binding = &mut normalized.binding;
			let entry = &definitions
				.get(&binding.target)
				.ok_or_else(|| Error::Invalid("snapshot lacks a bound definition".into()))?
				.definition;
			let kind = match binding.kind {
				BindingKind::Tool => "tool",
				BindingKind::Bundle => "bundle",
				BindingKind::Skill => "skill",
				BindingKind::Memory => "memory",
				BindingKind::Source => "source",
			};
			if entry.kind != kind {
				return Err(Error::Invalid(
					"snapshot Binding has a different kind".into(),
				));
			}
			if binding.kind == BindingKind::Bundle {
				if !ancestry.insert(binding.target.clone()) {
					return Err(Error::Invalid("recursive snapshot bundle expansion".into()));
				}
				let bundle: BundleConfig = serde_json::from_value(entry.config.clone())?;
				bundle.validate()?;
				if binding
					.members
					.iter()
					.any(|id| !bundle.members.iter().any(|r| &r.id == id))
				{
					return Err(Error::Invalid(
						"snapshot selects an undeclared bundle member".into(),
					));
				}
				for member in bundle
					.members
					.into_iter()
					.filter(|r| binding.members.is_empty() || binding.members.contains(&r.id))
				{
					let kind = match definitions.get(&member).map(|d| d.definition.kind.as_str()) {
						Some("tool") => BindingKind::Tool,
						Some("bundle") => BindingKind::Bundle,
						_ => {
							return Err(Error::Invalid(
								"snapshot bundle member is not a Tool or bundle".into(),
							));
						}
					};
					pending.push((
						NormalizedBinding {
							binding: Binding {
								kind,
								target: member,
								alias: None,
								narrow: binding.narrow.clone(),
								members: vec![],
							},
							origin: normalized.origin,
						},
						ancestry.clone(),
						None,
					));
				}
				continue;
			}
			if binding.kind == BindingKind::Tool {
				let descriptor: ToolDescriptor = serde_json::from_value(entry.config.clone())?;
				descriptor.declared_contract(binding.target.clone())?;
				validate_restrictions(&descriptor.operation, &binding.narrow)?;
				binding.narrow = descriptor.narrow.intersect(&binding.narrow)?;
				if let Some((provider, operation)) = lifecycle
					&& (descriptor.provider != provider
						|| descriptor.operation != operation
						|| descriptor.lifecycle.is_some())
				{
					return Err(Error::Invalid(
						"snapshot lifecycle companion has a different operation".into(),
					));
				}
				let alias = binding
					.alias
					.get_or_insert_with(|| descriptor.default_alias.clone());
				validate_alias(alias)?;
				if (normalized.origin == BindingOrigin::Required
					|| reserved_aliases().contains(alias.as_str()))
					&& alias != &descriptor.operation
				{
					return Err(Error::Invalid(
						"snapshot changes a reserved operation alias".into(),
					));
				}
				if let Some(companions) = descriptor.lifecycle {
					for (companion, suffix) in
						[(companions.poll, "poll"), (companions.cancel, "cancel")]
					{
						let operation = format!(
							"{}_{suffix}",
							if descriptor.operation == "shell" {
								"shell"
							} else {
								"python"
							}
						);
						pending.push((
							NormalizedBinding {
								binding: Binding::tool(companion),
								origin: BindingOrigin::Companion,
							},
							BTreeSet::new(),
							Some((descriptor.provider.clone(), operation)),
						));
					}
				}
			} else {
				if binding.narrow != Narrowing::default() {
					return Err(Error::Invalid(
						"snapshot context restriction is unsupported".into(),
					));
				}
				if matches!(binding.kind, BindingKind::Memory | BindingKind::Source) {
					let descriptor: sources::NativeContext =
						serde_json::from_value(entry.config.clone())?;
					descriptor.validate(kind)?;
					source_skill_support |= descriptor.requires_skill_support();
				}
			}
			if let Some(existing) = expected.get(&binding.target) {
				if (existing.origin == BindingOrigin::Companion
					|| normalized.origin == BindingOrigin::Companion)
					&& existing.binding == *binding
				{
					if existing.origin == BindingOrigin::Companion {
						expected.insert(binding.target.clone(), normalized);
					}
					continue;
				}
				return Err(Error::Invalid(
					"conflicting snapshot Binding expansion".into(),
				));
			}
			expected.insert(binding.target.clone(), normalized);
			if expected.len() > MAX_BINDINGS {
				return Err(Error::Invalid(
					"snapshot Binding closure exceeds 128 entries".into(),
				));
			}
		}
		if source_skill_support {
			for operation in SKILL_TOOLS {
				let binding = expected
					.get_mut(&QualifiedRef::builtin(&self.agent.registry_node, operation))
					.ok_or_else(|| Error::Invalid("snapshot lacks native Skill support".into()))?;
				if binding.origin == BindingOrigin::Default {
					binding.origin = BindingOrigin::SkillSupport;
				}
			}
		}
		let mut seen = BTreeSet::new();
		if self.bindings.len() != expected.len()
			|| self.bindings.iter().any(|saved| {
				!seen.insert(&saved.identity)
					|| expected.get(&saved.identity).is_none_or(|expected| {
						saved.origin != expected.origin
							|| saved.alias != expected.binding.alias
							|| saved.narrow != expected.binding.narrow
					})
			}) {
			return Err(Error::Invalid(
				"Run Bindings differ from the normalized Agent closure".into(),
			));
		}
		Ok(())
	}
}
