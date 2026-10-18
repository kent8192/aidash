use super::*;
use crate::ports::registry::CoreToolCatalog;
use aidash_domain::{
	capabilities::CoreCapabilities, provider::ToolSpec, tool::providers::requires_runner,
};
use std::sync::Arc;
struct Provider {
	runner: bool,
}
impl CoreToolCatalog for Provider {
	fn specifications(&self, _: &CoreCapabilities) -> BTreeMap<String, ToolSpec> {
		SANDBOXED_GROUPS
			.iter()
			.flat_map(|(_, operations)| operations.iter())
			.chain(
				[
					"file_search",
					"file_read",
					"skill_list",
					"skill_load",
					"skill_read",
				]
				.iter(),
			)
			.map(|operation| {
				(
					operation.to_string(),
					ToolSpec {
						name: operation.to_string(),
						description: operation.to_string(),
						parameters: json!({"type":"object"}),
					},
				)
			})
			.collect()
	}
	fn provider_available(&self, descriptor: &ToolDescriptor) -> Result<()> {
		if requires_runner(&descriptor.operation) && !self.runner {
			Err(Error::Invalid(
				"PROVIDER_UNAVAILABLE: admitted runner required".into(),
			))
		} else {
			Ok(())
		}
	}
}
fn validation(runner: bool) -> DefinitionValidation {
	DefinitionValidation::new(Arc::new(Provider { runner }))
}
#[test]
fn sandboxed_and_research_groups_have_only_provider_owned_declarations_and_complete_lifecycles() {
	let validation = validation(true);
	let groups = declarations(&validation, "aidash://node").unwrap();
	assert_eq!(groups.len(), SANDBOXED_GROUPS.len() + RESEARCH_GROUPS.len());
	for group in groups.into_iter().take(SANDBOXED_GROUPS.len()) {
		assert_eq!(group.bundle.kind, "bundle");
		let bundle: BundleConfig = serde_json::from_value(group.bundle.config.clone()).unwrap();
		assert_eq!(bundle.members.len(), group.operations.len());
		for entry in &group.operations {
			let descriptor: ToolDescriptor = serde_json::from_value(entry.config.clone()).unwrap();
			assert_eq!(descriptor.tier, ToolTier::Sandboxed);
			assert!(descriptor.transport.is_none());
			if let Some(lifecycle) = descriptor.lifecycle {
				for member in [lifecycle.poll, lifecycle.cancel] {
					assert!(bundle.members.contains(&member));
				}
			}
		}
		group.verify_provider(&validation).unwrap();
	}
}
#[test]
fn a_deployment_without_a_runner_offers_no_runner_groups() {
	let validation = validation(false);
	let offered: Vec<_> = offered(&validation, &validation, "aidash://node")
		.unwrap()
		.into_iter()
		.map(|group| group.name)
		.collect();
	assert!(!offered.contains(&"shell".to_owned()));
	assert!(!offered.contains(&"python".to_owned()));
	assert!(offered.contains(&"outbound_get".to_owned()));
	assert!(!offered.contains(&"task_assign".to_owned()));
}

#[test]
fn research_groups_declare_interchangeable_provider_operations() {
	use aidash_domain::tool::{Yield, providers::research_operation};
	let validation = validation(true);
	let groups = declarations(&validation, "aidash://node").unwrap();
	let research = groups
		.iter()
		.filter(|group| RESEARCH_GROUPS.iter().any(|(name, _)| *name == group.name))
		.collect::<Vec<_>>();
	assert_eq!(research.len(), 3);
	let mut aliases = BTreeMap::<String, Vec<(String, serde_json::Value)>>::new();
	for group in research {
		group.verify_provider(&validation).unwrap();
		let bundle: BundleConfig = serde_json::from_value(group.bundle.config.clone()).unwrap();
		assert_eq!(bundle.members.len(), group.operations.len());
		for entry in &group.operations {
			let descriptor: ToolDescriptor = serde_json::from_value(entry.config.clone()).unwrap();
			assert!(descriptor.transport.is_none() && descriptor.lifecycle.is_none());
			let (operation, tier) =
				research_operation(&descriptor.provider, &descriptor.operation).unwrap();
			assert_eq!(descriptor.tier, tier);
			let contract = descriptor
				.declared_contract(QualifiedRef {
					registry_node: "aidash://node".into(),
					id: entry.id.clone(),
					version: entry.version.clone(),
				})
				.unwrap();
			assert_eq!(
				contract.behavior.yields(Yield::Citations),
				operation != aidash_domain::research::Operation::Search
			);
			aliases
				.entry(descriptor.default_alias.clone())
				.or_default()
				.push((descriptor.provider.clone(), entry.schema.clone()));
		}
	}
	// The fixture provider offers every alias with the same schema as the
	// production providers, so it replaces them without an Agent change.
	assert_eq!(
		aliases.keys().cloned().collect::<Vec<_>>(),
		["research_fetch", "research_find", "research_search"]
	);
	for providers in aliases.values() {
		assert!(
			providers
				.iter()
				.any(|(provider, _)| provider == "research.fixture@1")
		);
		assert!(providers.windows(2).all(|pair| pair[0].1 == pair[1].1));
	}
}

#[test]
fn owners_cannot_declare_research_node_providers() {
	let entry = Entry {
		binding_normalization: None,
		installation: None,
		id: "owner.search".into(),
		version: "1.0.0".into(),
		kind: "tool".into(),
		name: BTreeMap::new(),
		description: BTreeMap::new(),
		capabilities: vec![],
		tags: vec![],
		languages: vec![],
		skills: vec![],
		schema: json!({}),
		config: serde_json::to_value(
			aidash_domain::tool::providers::research_descriptor(
				"aidash://node",
				"research.brave@1",
				"search",
			)
			.unwrap(),
		)
		.unwrap(),
	};
	assert!(matches!(
		crate::registry::system::reject_owner_definition(&entry),
		Err(Error::Forbidden)
	));
}
