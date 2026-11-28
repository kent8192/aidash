//! Unit tests for services::management.

#[rstest::rstest]
fn management_schema_has_typed_results() {
	let api = serde_json::to_value(crate::config::openapi::openapi().unwrap()).unwrap();
	for expected in [
		"core_agent_configure",
		"core_approval_list",
		"core_approval_decide",
		"core_approval_revoke",
		"core_grant_revoke",
		"reference_revoke",
		"core_operation_history",
		"core_thread_delete",
	] {
		let operation = api["paths"]
			.as_object()
			.unwrap()
			.values()
			.flat_map(|path| path.as_object().unwrap().values())
			.find(|op| op["operationId"] == expected)
			.unwrap_or_else(|| panic!("missing {expected}"));
		let response = &operation["responses"]["200"]["content"]["application/json"]["schema"];
		assert!(
			response["$ref"].is_string(),
			"{expected} must have an executable typed response: {response}"
		);
	}
}
