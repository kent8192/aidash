use super::*;
use serde_json::json;

/// An agent whose id and version together take `total` bytes.
fn agent(total: usize) -> Entry {
	let version = format!("1.0.0-{}", "a".repeat(total - 100 - "1.0.0-".len()));
	serde_json::from_value(json!({
		"id": "a".repeat(100),
		"version": version,
		"kind": "agent",
		"name": {"en": "Agent"},
		"description": {"en": "Agent"},
		"config": {}
	}))
	.unwrap()
}

#[test]
fn an_agent_subject_fits_the_longest_node_address() {
	let room = 256 - crate::nodes::NodeAddress::MAX_LEN - "/agents/@".len();
	validate_metadata(&agent(room), true).unwrap();
	let error = validate_metadata(&agent(room + 1), true).unwrap_err();
	assert!(error.to_string().contains("exceeds 256 bytes"), "{error}");
}
