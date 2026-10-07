use super::*;
use rstest::{fixture, rstest};
use serde_json::json;

#[fixture]
pub(super) fn inspection() -> Inspection {
	serde_json::from_value(json!({"node_id":"aidash://receiver","authority_digest":"sha256:pinned",
        "agent":{"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},"description":{"en":""}},
        "definitions":[],"binding_snapshot":{"schema_version":1,"agent":{"registry_node":"aidash://receiver","id":"agent","version":"1.0.0"},"remote":true,"bindings":[],"definitions":[]}})).unwrap()
}
#[rstest]
#[case(0, 0, true)]
#[case(0, 1, true)]
#[case(1, 1, true)]
#[case(1, 0, false)]
#[case(1, 2, false)]
fn only_an_unrequested_semantic_version_is_a_matching_wildcard(
	inspection: Inspection,
	#[case] pinned_version: u32,
	#[case] fresh_version: u32,
	#[case] matches: bool,
) {
	let mut pinned = inspection.clone();
	pinned.semantic_memory = pinned_version;
	let mut fresh = inspection;
	fresh.semantic_memory = fresh_version;
	assert_eq!(fresh.satisfies(&pinned), matches);
}
#[rstest]
#[case("node_id",json!("aidash://other"))]
#[case("authority_digest",json!("sha256:changed"))]
#[case("generation",json!({"revision":2}))]
#[case("compactor",json!({"id":"compactor","version":"1.0.0"}))]
fn every_pinned_authority_or_provider_change_breaks_matching(
	inspection: Inspection,
	#[case] field: &str,
	#[case] changed: serde_json::Value,
) {
	let mut wire = serde_json::to_value(&inspection).unwrap();
	wire[field] = changed;
	let fresh: Inspection = serde_json::from_value(wire).unwrap();
	assert!(!fresh.satisfies(&inspection));
}
#[rstest]
fn pinned_agent_metadata_and_definitions_are_exact(inspection: Inspection) {
	let mut changed = inspection.clone();
	changed
		.agent
		.description
		.insert("en".into(), "Changed".into());
	assert!(!changed.satisfies(&inspection));
	let mut changed = inspection.clone();
	changed.definitions.push(Definition {
		entry: EntityRef {
			id: "extra".into(),
			version: "1.0.0".into(),
		},
		kind: "agent".into(),
		digest: "digest".into(),
		metadata: inspection.agent.clone(),
	});
	assert!(!changed.satisfies(&inspection));
}
#[rstest]
fn optional_protocol_extensions_keep_their_legacy_omission(inspection: Inspection) {
	let wire = serde_json::to_value(&inspection).unwrap();
	for field in ["generation", "lineage", "semantic_memory", "compactor"] {
		assert!(wire.get(field).is_none());
	}
	let mut changed = wire;
	changed["caller_authority"] = json!(true);
	assert!(serde_json::from_value::<Inspection>(changed).is_err());
}
