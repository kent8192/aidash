use super::*;
use crate::{
	ports::{Credentials, registry::CoreToolCatalog},
	registry::DefinitionValidation,
};
use aidash_domain::{capabilities::CoreCapabilities, provider::ToolSpec, registry::Entry};
use rstest::{fixture, rstest};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};

struct ReceiverOnlySecrets;
impl Credentials for ReceiverOnlySecrets {
	fn resolve(&self, _: &str) -> Result<String> {
		panic!("source inspection must not resolve receiver credentials")
	}
}
struct CoreContracts;
impl CoreToolCatalog for CoreContracts {
	fn specifications(&self, _: &CoreCapabilities) -> BTreeMap<String, ToolSpec> {
		BTreeMap::new()
	}
}
#[fixture]
fn validation() -> DefinitionValidation {
	DefinitionValidation::new(Arc::new(ReceiverOnlySecrets), Arc::new(CoreContracts))
}
#[fixture]
fn inspection() -> Inspection {
	let agent: Entry = serde_json::from_value(
		json!({"id":"a","version":"1.0.0","kind":"agent","name":{"en":"A"},"description":{"en":""},
        "config":{"model":{"id":"m","version":"1.0.0"},"instructions":"Do work."}}),
	)
	.unwrap();
	let model: Entry = serde_json::from_value(
		json!({"id":"m","version":"1.0.0","kind":"model","name":{"en":"M"},"description":{"en":""},
        "config":{"credential_env":"RECEIVER_ONLY","endpoint":"https://receiver-only.invalid"}}),
	)
	.unwrap();
	Inspection {
		generation: None,
		lineage: vec![],
		node_id: "aidash://receiver".into(),
		authority_digest: format!("sha256:{}", "a".repeat(64)),
		agent: agent.clone(),
		definitions: [agent, model]
			.into_iter()
			.map(|entry| aidash_domain::federation::execution::Definition {
				entry: EntityRef {
					id: entry.id.clone(),
					version: entry.version.clone(),
				},
				kind: entry.kind.clone(),
				digest: digest(&serde_json::to_value(&entry).unwrap()),
				metadata: entry,
			})
			.collect(),
		semantic_memory: 0,
		compactor: None,
	}
}
fn expected() -> EntityRef {
	EntityRef {
		id: "a".into(),
		version: "1.0.0".into(),
	}
}
#[rstest]
fn exact_receiver_dependencies_do_not_resolve_remote_credentials(
	validation: DefinitionValidation,
	inspection: Inspection,
) {
	validate_inspection(
		&validation,
		&inspection,
		"aidash://receiver",
		&expected(),
		&Search::default(),
	)
	.unwrap();
}
#[rstest]
#[case("node")]
#[case("authority_digest")]
#[case("agent")]
#[case("metadata_id")]
#[case("metadata_kind")]
#[case("definition_digest")]
#[case("missing_dependency")]
#[case("duplicate_dependency")]
#[case("conflicting_dependency_kind")]
fn rebound_or_incomplete_receiver_definitions_are_rejected(
	validation: DefinitionValidation,
	mut inspection: Inspection,
	#[case] mutation: &str,
) {
	match mutation {
		"node" => inspection.node_id = "aidash://other".into(),
		"authority_digest" => inspection.authority_digest = "sha256:short".into(),
		"agent" => inspection.agent.id = "other".into(),
		"metadata_id" => inspection.definitions[1].metadata.id = "other".into(),
		"metadata_kind" => inspection.definitions[1].metadata.kind = "tool".into(),
		"definition_digest" => {
			inspection.definitions[1].digest = format!("sha256:{}", "b".repeat(64))
		}
		"missing_dependency" => {
			inspection.definitions.pop();
		}
		"duplicate_dependency" => inspection.definitions[1] = inspection.definitions[0].clone(),
		"conflicting_dependency_kind" => {
			inspection.agent.config["tools"] = json!([{"id":"m","version":"1.0.0"}])
		}
		_ => panic!("unknown mutation"),
	}
	assert!(matches!(
		validate_inspection(
			&validation,
			&inspection,
			"aidash://receiver",
			&expected(),
			&Search::default()
		),
		Err(Error::External(_))
	));
}
#[rstest]
fn receiver_agent_must_match_the_requested_capability(
	validation: DefinitionValidation,
	inspection: Inspection,
) {
	let requirements = Search {
		capability: Some("unadvertised".into()),
		..Default::default()
	};
	assert!(matches!(
		validate_inspection(
			&validation,
			&inspection,
			"aidash://receiver",
			&expected(),
			&requirements
		),
		Err(Error::External(_))
	));
}
