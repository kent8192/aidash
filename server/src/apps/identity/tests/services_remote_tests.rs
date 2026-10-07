use super::*;
use crate::registry::digest;
#[rstest::rstest]
fn untrusted_inspections_cannot_substitute_or_omit_executor_definitions() {
	let reference = EntityRef {
		id: "agent".into(),
		version: "1.0.0".into(),
	};
	let entry: crate::registry::Entry = serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},"description":{"en":"Fixture"},"config":{"model":{"id":"model","version":"1.0.0"},"instructions":"Fixture","schema_version":1,"bindings":[],"remove_default":aidash_domain::registry::bindings::DEFAULT_TOOLS}})).unwrap();
	let model: crate::registry::Entry = serde_json::from_value(json!({"id":"model","version":"1.0.0","kind":"model","name":{"en":"Model"},"description":{"en":"Fixture"},"config":{}})).unwrap();
	use aidash_domain::registry::bindings::{
		BindingOrigin, BindingSnapshot, QualifiedRef, ResolvedBinding, ResolvedDefinition,
	};
	let node = "aidash://host";
	let mut entries = vec![entry.clone(), model];
	entries.extend(
		aidash_application::registry::system::entries(
			&crate::bootstrap::registry_validation(),
			node,
		)
		.unwrap()
		.into_iter()
		.filter(|e| {
			matches!(
				e.id.as_str(),
				"aidash.workspace_read" | "aidash.human_request"
			)
		}),
	);
	let definitions = entries
		.into_iter()
		.map(|definition| {
			ResolvedDefinition::new(
				QualifiedRef {
					registry_node: node.into(),
					id: definition.id.clone(),
					version: definition.version.clone(),
				},
				definition,
			)
			.unwrap()
		})
		.collect::<Vec<_>>();
	let bindings = definitions
		.iter()
		.filter(|d| d.definition.kind == "tool")
		.map(|d| {
			let descriptor: aidash_domain::tool::providers::ToolDescriptor =
				serde_json::from_value(d.definition.config.clone()).unwrap();
			let contract = descriptor.declared_contract(d.identity.clone()).unwrap();
			ResolvedBinding {
				identity: d.identity.clone(),
				definition: d.definition.clone(),
				digest: d.digest.clone(),
				origin: BindingOrigin::Required,
				alias: Some(descriptor.operation),
				narrow: Default::default(),
				installation: None,
				provider_contract_digest: Some(digest(&serde_json::to_value(contract).unwrap())),
				provider_implementation: Some(format!("{}:native", descriptor.provider)),
				excluded_reason: None,
			}
		})
		.collect();
	let snapshot = BindingSnapshot {
		schema_version: 1,
		agent: definitions[0].identity.clone(),
		remote: true,
		bindings,
		definitions: definitions.clone(),
		foreign_agents: vec![],
	};
	let mut inspection: Inspection = serde_json::from_value(json!({"node_id":node,"authority_digest":digest(&json!({})),"agent":entry,"binding_snapshot":snapshot,"definitions":definitions.iter().map(|d| json!({"entry":d.identity.local(),"kind":d.definition.kind,"digest":d.digest,"metadata":d.definition})).collect::<Vec<_>>()})).unwrap();
	assert!(validate(&inspection, "aidash://host", &reference, &Search::default()).is_ok());
	assert!(
		validate(
			&inspection,
			"aidash://other",
			&reference,
			&Search::default()
		)
		.is_err()
	);
	let original = inspection.clone();
	inspection.definitions.pop();
	assert!(validate(&inspection, "aidash://host", &reference, &Search::default()).is_err());
	inspection = original.clone();
	inspection.definitions[1] = inspection.definitions[0].clone();
	assert!(validate(&inspection, "aidash://host", &reference, &Search::default()).is_err());
	inspection = original.clone();
	inspection
		.agent
		.description
		.insert("en".into(), "Substituted".into());
	assert!(validate(&inspection, "aidash://host", &reference, &Search::default()).is_err());
	inspection = original;
	inspection.definitions[1].digest = format!("sha256:{}", "é".repeat(32));
	assert!(validate(&inspection, "aidash://host", &reference, &Search::default()).is_err());
}
