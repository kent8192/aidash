//! System declarations cover the builtins of every Exposure policy.
use super::*;
use crate::ports::{
	Credentials,
	registry::{CoreToolCatalog, DefinitionLookup, DefinitionWriter},
};
use aidash_domain::decision::{Hook, Mode};
use aidash_domain::{
	capabilities::CoreCapabilities,
	provider::ToolSpec,
	registry::EntityRef,
	tool::providers::{ToolDescriptor, core_provider},
};
use std::{collections::BTreeMap, sync::Arc};

const NODE: &str = "aidash://local";

/// Native core tools live outside the application builtin table.
struct Contracts;
impl Credentials for Contracts {
	fn resolve(&self, _: &str) -> Result<String> {
		panic!("seeding cannot disclose credentials")
	}
}
impl CoreToolCatalog for Contracts {
	fn specifications(&self, _: &CoreCapabilities) -> BTreeMap<String, ToolSpec> {
		[
			"skill_list",
			"skill_load",
			"skill_read",
			"file_search",
			"file_read",
		]
		.into_iter()
		.chain(
			packages::SANDBOXED_GROUPS
				.iter()
				.flat_map(|(_, operations)| operations.iter().copied()),
		)
		.map(|name| {
			(
				name.to_owned(),
				ToolSpec {
					name: name.into(),
					description: format!("Native {name}"),
					parameters: json!({"type":"object"}),
				},
			)
		})
		.collect()
	}
}

#[test]
fn seeding_declares_exposure_builtins_beside_the_unchanged_legacy_set() {
	let validation = DefinitionValidation::new(Arc::new(Contracts));
	let entries = entries(&validation, NODE).unwrap();
	assert_eq!(
		entries.len(),
		REQUIRED_TOOLS.len() + DEFAULT_TOOLS.len() + EXPOSURE_TOOLS.len() + 2
	);
	let specifications = validation.node_specifications();
	for entry in &entries {
		let operation = entry.id.strip_prefix("aidash.").unwrap();
		let descriptor: ToolDescriptor = serde_json::from_value(entry.config.clone()).unwrap();
		assert_eq!(Some(descriptor.provider.as_str()), core_provider(operation));
		assert!(is_builtin(entry));
		assert!(builtin_reference(&EntityRef {
			id: entry.id.clone(),
			version: entry.version.clone(),
		}));
		let specification = &specifications[operation];
		assert_eq!(entry.description["en"], specification.description);
		assert_eq!(entry.schema, specification.parameters);
		if EXPOSURE_TOOLS.contains(&operation) || operation == SKILL_ASSET_READ {
			assert_eq!(
				entry.description["en"],
				crate::tools::builtins()[operation].description
			);
		}
	}
	// Legacy declarations keep their position and bytes ahead of the new ones.
	assert_eq!(
		entries[..REQUIRED_TOOLS.len() + DEFAULT_TOOLS.len()]
			.iter()
			.map(|entry| entry.id.as_str())
			.collect::<Vec<_>>(),
		REQUIRED_TOOLS
			.iter()
			.chain(DEFAULT_TOOLS)
			.map(|operation| format!("aidash.{operation}"))
			.collect::<Vec<_>>()
	);
	for operation in EXPOSURE_TOOLS {
		let entry = entries
			.iter()
			.find(|entry| entry.id == format!("aidash.{operation}"))
			.unwrap();
		let descriptor: ToolDescriptor = serde_json::from_value(entry.config.clone()).unwrap();
		assert_eq!(descriptor.provider, "core.exposure@1");
	}
	assert!(!builtin_reference(&EntityRef {
		id: "aidash.capability_load".into(),
		version: "2.0.0".into(),
	}));
}

#[test]
fn every_tenant_is_declared_the_builtins_and_the_approval_decider() {
	let validation = DefinitionValidation::new(Arc::new(Contracts));
	let builtins = entries(&validation, NODE).unwrap();

	let declared = declarations(&validation, NODE).unwrap();

	assert_eq!(declared.len(), builtins.len() + 3);
	assert_eq!(declared[..builtins.len()], builtins);
	assert_eq!(declared[builtins.len()], approval_decider().unwrap());
	assert_eq!(
		declared[builtins.len() + 1],
		aidash_domain::registry::bindings::source_defaults::workspace_observation().unwrap()
	);
	assert_eq!(declared.last(), Some(&compaction_decider().unwrap()));
	assert!(
		builtins.iter().all(|entry| entry.id != APPROVAL_DECIDER_ID),
		"only a built-in tool is admitted without a Catalog Approval"
	);
}

#[test]
fn the_approval_decider_is_a_valid_immutable_deployment_definition() {
	let entry = approval_decider().unwrap();
	assert_eq!(
		(entry.id.as_str(), entry.version.as_str()),
		("aidash.approval-decider", "1.0.0")
	);
	DefinitionValidation::new(Arc::new(Contracts))
		.validate_in(&entry, false)
		.unwrap();
	assert!(is_deployment_definition(&entry));
	assert_eq!(attributes(&entry)["immutable"], true);
	// No Tenant publishes or replaces an `aidash.` definition.
	assert!(matches!(
		reject_owner_definition(&entry),
		Err(crate::Error::Forbidden)
	));
	assert!(!is_builtin(&entry));

	let config: DeciderConfig = serde_json::from_value(entry.config).unwrap();
	assert_eq!(config.hook, Hook::Approval);
	assert_eq!(config.mode, Mode::Enforce);
	assert_eq!(config.max_calls_per_run, 200);
	assert!(!config.state_retention.enabled);
	assert_eq!(config.provider_credential, "openrouter");
	assert_eq!(config.models(), ["typesafe/jev-1.14", "cloudflare/clef"]);
	assert_eq!(
		config.routes[1].hosts,
		["cloudflare", "deepinfra", "primeintellect"]
	);
	assert!(
		config.routes[1]
			.state_budget
			.long_context_request_bytes
			.is_none()
	);
	assert_eq!(config.routes[1].state_budget.default_request_bytes, 16_000);
}

#[derive(Default)]
struct Writer {
	entries: BTreeMap<(String, String), Entry>,
}
#[async_trait::async_trait]
impl DefinitionLookup for Writer {
	async fn definition(&mut self, id: &str, version: &str) -> Result<Entry> {
		self.entries
			.get(&(id.into(), version.into()))
			.cloned()
			.ok_or_else(|| crate::Error::NotFound(id.into()))
	}
}
#[async_trait::async_trait]
impl DefinitionWriter for Writer {
	async fn insert_definition(&mut self, entry: &Entry) -> Result<bool> {
		let key = (entry.id.clone(), entry.version.clone());
		Ok(self.entries.insert(key, entry.clone()).is_none())
	}
}

#[tokio::test]
async fn seeding_adds_the_approval_decider_to_every_registry_and_repeats_safely() {
	let validation = DefinitionValidation::new(Arc::new(Contracts));
	let mut writer = Writer::default();
	seed(&mut writer, &validation, NODE).await.unwrap();
	let seeded = writer.entries.len();
	let decider = writer
		.definition(APPROVAL_DECIDER_ID, APPROVAL_DECIDER_VERSION)
		.await
		.unwrap();
	assert_eq!(decider, approval_decider().unwrap());
	let source =
		aidash_domain::registry::bindings::source_defaults::workspace_observation().unwrap();
	assert_eq!(
		writer
			.definition(&source.id, &source.version)
			.await
			.unwrap(),
		source
	);
	assert_eq!(
		writer
			.definition(COMPACTION_DECIDER_ID, COMPACTION_DECIDER_VERSION)
			.await
			.unwrap(),
		compaction_decider().unwrap()
	);
	// The tools stay as they were: the Decider is one definition beyond them.
	assert!(seeded > entries(&validation, NODE).unwrap().len());
	seed(&mut writer, &validation, NODE).await.unwrap();
	assert_eq!(writer.entries.len(), seeded);
	// Another document under the same identity is a conflict, not a replacement.
	let mut changed = decider.clone();
	changed.description.insert("en".into(), "Other".into());
	writer
		.entries
		.insert((changed.id.clone(), changed.version.clone()), changed);
	assert!(matches!(
		seed(&mut writer, &validation, NODE).await,
		Err(crate::Error::Conflict(_))
	));
}

struct ResearchContracts;
impl CoreToolCatalog for ResearchContracts {
	fn specifications(&self, capabilities: &CoreCapabilities) -> BTreeMap<String, ToolSpec> {
		Contracts.specifications(capabilities)
	}
	fn provider_available(&self, descriptor: &ToolDescriptor) -> Result<()> {
		if descriptor.provider.starts_with("research.") {
			Ok(())
		} else {
			Err(crate::Error::Forbidden)
		}
	}
}

#[rstest::rstest]
#[case::before_page_images(false)]
#[case::already_expanded_legacy_bundle(true)]
#[tokio::test]
async fn adding_page_views_preserves_existing_immutable_research_bundle_bytes(
	#[case] had_view: bool,
) {
	let validation = DefinitionValidation::new(Arc::new(ResearchContracts));
	let groups = packages::declarations(&validation, NODE).unwrap();
	let mut writer = Writer::default();
	let mut previous = vec![];
	for group in groups
		.iter()
		.filter(|group| matches!(group.name.as_str(), "research_page" | "research_fixture"))
	{
		let mut old = group.bundle.clone();
		old.version = "1.0.0".into();
		if !had_view {
			old.config["members"]
				.as_array_mut()
				.unwrap()
				.retain(|member| !member["id"].as_str().unwrap().ends_with(".view"));
			old.capabilities
				.retain(|capability| capability != "research.view");
		}
		writer
			.entries
			.insert((old.id.clone(), old.version.clone()), old.clone());
		previous.push(old);
	}
	seed(&mut writer, &validation, NODE).await.unwrap();
	seed(&mut writer, &validation, NODE).await.unwrap();
	for old in previous {
		assert_eq!(writer.entries[&(old.id.clone(), "1.0.0".into())], old);
		let current = &writer.entries[&(old.id.clone(), "1.1.0".into())];
		assert!(
			current.config["members"]
				.as_array()
				.unwrap()
				.iter()
				.any(|member| member["id"].as_str().unwrap().ends_with(".view"))
		);
	}
}

#[test]
fn compaction_declaration_pins_the_evaluated_routes_without_granting_authority() {
	let entry = compaction_decider().unwrap();
	DefinitionValidation::new(Arc::new(Contracts))
		.validate_in(&entry, false)
		.unwrap();
	assert!(!is_builtin(&entry));
	assert!(is_deployment_definition(&entry));
	assert!(matches!(
		reject_owner_definition(&entry),
		Err(crate::Error::Forbidden)
	));
	let config: DeciderConfig = serde_json::from_value(entry.config).unwrap();
	assert_eq!(config.hook, Hook::Compaction);
	assert_eq!(config.mode, Mode::Enforce);
	assert_eq!(config.max_calls_per_run, 100);
	assert_eq!(config.routes[0], DeciderRoute::jev("typesafe/jev-1.14"));
	let fallback = &config.routes[1];
	assert_eq!(fallback.model, "cloudflare/clef");
	assert_eq!(fallback.threshold.value(), 0.15);
	assert_eq!(
		fallback.hosts,
		["cloudflare", "deepinfra", "primeintellect"]
	);
	assert_eq!(fallback.state_budget.default_request_bytes, 16_000);
	assert_eq!(fallback.state_budget.default_state_bytes, 12_000);
	assert!(fallback.state_budget.long_context_request_bytes.is_none());
	assert!(!config.state_retention.enabled);
}

#[test]
fn required_workspace_source_is_builtin_only_at_its_exact_canonical_definition() {
	let mut source =
		aidash_domain::registry::bindings::source_defaults::workspace_observation().unwrap();
	assert!(is_builtin(&source));
	assert!(builtin_reference(&EntityRef {
		id: source.id.clone(),
		version: source.version.clone()
	}));
	source.config["source"]["adapter"] = json!("workspace_retrieval");
	assert!(
		!is_builtin(&source),
		"a source may not claim mandatory builtin authority with changed provider bytes"
	);
	assert!(!builtin_reference(&EntityRef {
		id: source.id,
		version: "2.0.0".into()
	}));
}
