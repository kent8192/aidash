//! System declarations cover the builtins of every Exposure policy.
use super::*;
use crate::ports::{Credentials, registry::CoreToolCatalog};
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
	let validation = DefinitionValidation::new(Arc::new(Contracts), Arc::new(Contracts));
	let entries = entries(&validation, NODE).unwrap();
	assert_eq!(
		entries.len(),
		REQUIRED_TOOLS.len() + DEFAULT_TOOLS.len() + EXPOSURE_TOOLS.len() + 1
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
