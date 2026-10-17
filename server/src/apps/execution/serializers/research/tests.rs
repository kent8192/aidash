use super::*;
use serde_json::json;

#[test]
fn retention_rules_are_cleared_only_by_an_explicit_null() {
	let cleared: RetentionInput = serde_json::from_value(json!({"rules": null})).unwrap();
	assert!(cleared.rules.is_none());
	// An omitted field is a malformed request, never a request to clear.
	assert!(serde_json::from_value::<RetentionInput>(json!({})).is_err());
}
