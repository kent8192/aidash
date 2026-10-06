//! Unit tests for services::management.
use super::*;
use serde_json::json;

#[test]
fn management_schema_has_typed_results_and_distinct_recipient_contracts() {
	let api = serde_json::to_value(crate::config::openapi::openapi().unwrap()).unwrap();
	let schemas = &api["components"]["schemas"];
	assert_eq!(schemas["Recipient"]["additionalProperties"], false);
	assert!(
		schemas["Recipient"]["properties"]
			.get("workspace_id")
			.is_none()
	);
	assert!(
		schemas["FileRecipient"]["required"]
			.as_array()
			.unwrap()
			.contains(&json!("workspace_id"))
	);
	for expected in [
		"core_agent_configure",
		"core_file_materialize",
		"core_approval_list",
		"core_approval_decide",
		"core_approval_revoke",
		"core_grant_revoke",
		"reference_revoke",
		"core_deletion_confirmation",
		"file_transfer_status",
		"file_transfer_history",
		"file_transfer_reconcile",
		"file_recipient_list",
		"core_operation_history",
		"core_thread_delete",
		"core_cleanup_reconcile",
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
	let _ = std::mem::size_of::<TransferStatus>();
}
