//! Unit tests for services::tools.
use super::*;
#[rstest::rstest]
fn output_envelope_schema_accepts_the_serialized_wire_contract() {
	use crate::capabilities::contracts::*;
	let envelope = Envelope {
		operation_id: "read-only-call".into(),
		status: "completed".into(),
		policy_revision: 1,
		area_id: uuid::Uuid::nil(),
		generation: 1,
		revision: 1,
		result: CapabilityResult::Search(SearchResult {
			matches: vec![],
			unavailable: vec![SearchUnavailable {
				file_id: uuid::Uuid::nil(),
				error: "REPRESENTATION_UNAVAILABLE".into(),
			}],
			next_cursor: None,
			truncated: false,
		}),
	};
	let wire = serde_json::to_value(&envelope).unwrap();
	let schema = schema::<Envelope>();
	jsonschema::validator_for(&schema)
		.unwrap()
		.validate(&wire)
		.unwrap_or_else(|e| panic!("{e}: {schema}"));
	assert!(serde_json::from_value::<Envelope>(wire).is_ok());
}
/// An Agent's tool result passes through `Envelope`, which keeps only what its
/// variant declares, so a field set on the operation can vanish on the way.
#[rstest::rstest]
fn a_timed_out_command_keeps_its_deadline_flag_on_the_way_to_the_model() {
	use crate::capabilities::contracts::*;
	let wire = serde_json::json!({
		"operation_id": "operation", "status": "failed", "policy_revision": 1,
		"area_id": uuid::Uuid::nil(), "generation": 1, "revision": 2,
		"kind": "shell", "epoch": 1, "termination_confirmed": true, "writer_frozen": false,
		"session_id": null, "displays": [], "exit_code": 143, "output": "", "next_offset": null,
		"truncated": false, "effects_may_have_occurred": true, "error": null,
		"deadline_expired": true,
	});
	let envelope: Envelope = serde_json::from_value(wire).unwrap();
	assert!(matches!(envelope.result, CapabilityResult::Runtime(_)));
	assert_eq!(
		serde_json::to_value(&envelope).unwrap()["deadline_expired"],
		true
	);
}
/// A poll tells the Agent how old the output it sees is, so that it can tell
/// a stalled view from a stalled command (#709). The result passes through the
/// same envelope as the deadline flag above.
#[rstest::rstest]
fn a_running_command_keeps_the_age_of_its_output_on_the_way_to_the_model() {
	use crate::capabilities::contracts::*;
	let mut wire = serde_json::json!({
		"operation_id": "operation", "status": "running", "policy_revision": 1,
		"area_id": uuid::Uuid::nil(), "generation": 1, "revision": 2,
		"kind": "shell", "epoch": 1, "termination_confirmed": false, "writer_frozen": false,
		"session_id": null, "displays": [], "exit_code": null, "output": "", "next_offset": null,
		"truncated": false, "effects_may_have_occurred": true, "error": null,
		"output_stale_seconds": 42,
	});
	let envelope: Envelope = serde_json::from_value(wire.clone()).unwrap();
	assert!(matches!(envelope.result, CapabilityResult::Runtime(_)));
	assert_eq!(
		serde_json::to_value(&envelope).unwrap()["output_stale_seconds"],
		42
	);
	// A result without an age stays without one rather than reading as zero.
	wire.as_object_mut().unwrap().remove("output_stale_seconds");
	let envelope: Envelope = serde_json::from_value(wire).unwrap();
	assert!(
		serde_json::to_value(&envelope)
			.unwrap()
			.get("output_stale_seconds")
			.is_none()
	);
}
#[rstest::rstest]
fn executable_contracts_are_derived_and_disabled_by_default() {
	let mut tools = BTreeMap::new();
	add(&mut tools, &CoreCapabilities::default());
	assert!(tools.is_empty());
	add(
		&mut tools,
		&CoreCapabilities {
			outbound: false,
			files: true,
			shell: true,
			python: true,
			patch: true,
			skills: true,
			sharing: true,
		},
	);
	assert_eq!(tools.len(), 16);
	for (name, tool) in &tools {
		let schema = tool.specification().parameters;
		assert!(!schema.to_string().contains("$ref"), "{name}");
		if name != "skill_read" {
			assert_eq!(schema["additionalProperties"], false, "{name}");
		}
	}
	assert!(!tools.contains_key("python_install"));
}

#[test]
fn area_mutation_tools_explain_where_to_find_the_expected_revision() {
	let tools = implementations();
	for name in ["shell", "code_interpreter", "apply_patch", "file_share"] {
		let spec = tools[name].specification();
		let description = spec.parameters["properties"]["expected_revision"]["description"]
			.as_str()
			.unwrap_or_default();
		assert!(
			description.contains("previous result"),
			"{name}: {description}"
		);
		assert!(description.contains("1"), "{name}: {description}");
		assert!(spec.description.contains("previous result"), "{name}");
	}
}
