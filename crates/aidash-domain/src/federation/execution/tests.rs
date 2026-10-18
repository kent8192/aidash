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
#[case("decider",json!({"id":"decider","version":"1.0.0"}))]
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
	for field in ["generation", "lineage", "semantic_memory", "decider"] {
		assert!(wire.get(field).is_none());
	}
	let mut changed = wire;
	changed["caller_authority"] = json!(true);
	assert!(serde_json::from_value::<Inspection>(changed).is_err());
}

#[rstest]
#[case::origin("registry_node", json!("aidash://other"))]
#[case::version("version", json!("2.0.0"))]
fn substituted_binding_snapshot_is_rejected_even_when_legacy_inspection_fields_match(
	inspection: Inspection,
	#[case] field: &str,
	#[case] value: serde_json::Value,
) {
	let mut changed = inspection.clone();
	let mut snapshot = serde_json::to_value(&changed.binding_snapshot).unwrap();
	snapshot["agent"][field] = value;
	changed.binding_snapshot = serde_json::from_value(snapshot).unwrap();
	assert!(!changed.satisfies(&inspection));
}

#[fixture]
fn described(inspection: Inspection) -> serde_json::Value {
	json!({
		"grant_id": Uuid::nil(),
		"source_node": "aidash://home",
		"target_node": "aidash://receiver",
		"source_tenant": "acme",
		"source_subject": "alice",
		"task": {"id":Uuid::nil(),"workspace_id":Uuid::nil(),"title":"Task","description":"Describe","status":"OPEN","requirements":{},"owner":null,"created_by":"human","dependencies":[],"parent_id":null,"revision":1,"created_at":"2026-10-02T00:00:00Z"},
		"inspection": inspection,
		"expires_at": "2026-10-02T00:00:00Z",
	})
}
#[rstest]
fn a_reply_without_a_corpus_digest_is_a_valid_older_protocol_reply(described: serde_json::Value) {
	let reply = Described::from_value(described.clone()).unwrap();
	assert_eq!(reply.corpus_digest, None);
	// The Description parses unchanged and re-serializes without the field.
	assert_eq!(serde_json::to_value(&reply).unwrap(), described);
}
#[rstest]
fn the_corpus_digest_is_split_from_the_description_it_would_otherwise_change(
	described: serde_json::Value,
) {
	let mut wire = described.clone();
	wire["corpus_digest"] = json!("sha256:corpus");
	let reply = Described::from_value(wire.clone()).unwrap();
	assert_eq!(reply.corpus_digest.as_deref(), Some("sha256:corpus"));
	// Admission and refresh compare the Description with the pinned copy; the
	// digest must not take part, or every corpus change would revoke the Run.
	assert_eq!(serde_json::to_value(&reply.description).unwrap(), described);
	assert_eq!(serde_json::to_value(&reply).unwrap(), wire);
}
#[rstest]
fn a_malformed_digest_or_an_unknown_description_field_is_rejected(described: serde_json::Value) {
	let mut wrong_type = described.clone();
	wrong_type["corpus_digest"] = json!(7);
	assert!(Described::from_value(wrong_type).is_err());
	let mut unknown = described;
	unknown["caller_authority"] = json!(true);
	assert!(Described::from_value(unknown).is_err());
}
