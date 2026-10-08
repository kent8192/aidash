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
#[rstest::rstest]
fn executable_contracts_are_derived_and_disabled_by_default() {
	let mut tools = BTreeMap::new();
	add(&mut tools, &CoreCapabilities::default());
	assert!(tools.is_empty());
	add(
		&mut tools,
		&CoreCapabilities {
			files: true,
			shell: true,
			python: true,
			patch: true,
			skills: true,
			sharing: true,
		},
	);
	assert_eq!(tools.len(), 15);
	for (name, tool) in &tools {
		let schema = tool.specification().parameters;
		assert!(!schema.to_string().contains("$ref"), "{name}");
		if name != "skill_read" {
			assert_eq!(schema["additionalProperties"], false, "{name}");
		}
	}
	let install = tools["python_install"].specification().parameters;
	assert_eq!(
		install["properties"]["wheels"]["items"]["additionalProperties"],
		false
	);
	assert!(
		install["required"]
			.as_array()
			.unwrap()
			.contains(&json!("wheels"))
	);
}
