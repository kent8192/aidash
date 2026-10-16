use super::*;
use crate::ports::{Credentials, registry::CoreToolCatalog};
use aidash_domain::{
	capabilities::CoreCapabilities, provider::ToolSpec, tool::providers::requires_runner,
};
use std::sync::Arc;
struct Credential;
impl Credentials for Credential {
	fn resolve(&self, _: &str) -> Result<String> {
		panic!("declarations must not load secrets")
	}
}
struct Provider {
	runner: bool,
}
impl CoreToolCatalog for Provider {
	fn specifications(&self, _: &CoreCapabilities) -> BTreeMap<String, ToolSpec> {
		HOST_GROUPS
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
	DefinitionValidation::new(Arc::new(Credential), Arc::new(Provider { runner }))
}
#[test]
fn six_host_and_research_groups_have_only_provider_owned_declarations_and_complete_lifecycles() {
	let validation = validation(true);
	let groups = declarations(
		&Principal::DeploymentAdministrator,
		&validation,
		"aidash://node",
	)
	.unwrap();
	assert_eq!(groups.len(), HOST_GROUPS.len() + RESEARCH_GROUPS.len());
	for group in groups.into_iter().take(HOST_GROUPS.len()) {
		assert_eq!(group.bundle.kind, "bundle");
		let bundle: BundleConfig = serde_json::from_value(group.bundle.config.clone()).unwrap();
		assert_eq!(bundle.members.len(), group.operations.len());
		for entry in &group.operations {
			let descriptor: ToolDescriptor = serde_json::from_value(entry.config.clone()).unwrap();
			assert_eq!(descriptor.tier, ToolTier::Host);
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
fn missing_runner_excludes_sandbox_defaults_without_fallback_and_other_owners_cannot_create_them() {
	let validation = validation(false);
	let selected = HOST_GROUPS
		.iter()
		.map(|(name, _)| name.to_string())
		.collect::<Vec<_>>();
	let plan = configured_defaults(
		&Principal::DeploymentAdministrator,
		&validation,
		&validation,
		"aidash://node",
		&selected,
	)
	.unwrap();
	assert_eq!(plan.pending.len(), 4);
	assert_eq!(
		plan.unavailable.keys().cloned().collect::<Vec<_>>(),
		vec!["python", "shell"]
	);
	assert!(
		plan.unavailable
			.values()
			.all(|reason| reason.contains("PROVIDER_UNAVAILABLE"))
	);
	assert!(
		declarations(
			&Principal::Subject {
				tenant: "deployment_administrator".into(),
				subject: "deployment_administrator".into()
			},
			&validation,
			"aidash://node"
		)
		.is_err()
	);
	assert!(
		configured_defaults(
			&Principal::DeploymentAdministrator,
			&validation,
			&validation,
			"aidash://node",
			&["unknown".into()]
		)
		.is_err()
	);
}

#[test]
fn research_groups_declare_interchangeable_provider_operations() {
	use aidash_domain::tool::{Yield, providers::research_operation};
	let validation = validation(true);
	let groups = declarations(
		&Principal::DeploymentAdministrator,
		&validation,
		"aidash://node",
	)
	.unwrap();
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
