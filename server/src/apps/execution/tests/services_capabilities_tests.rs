use super::*;
use serde_json::json;

#[rstest::rstest]
fn positive_utf8_chunk_limits_return_at_least_one_complete_character() {
	let text = "界x";
	assert_eq!(bounded_utf8_end(text, 0, 0), 0);
	let end = bounded_utf8_end(text, 0, 1);
	assert_eq!(&text[..end], "界");
	assert_eq!(bounded_utf8_end(text, 0, 2), 3);
	assert_eq!(bounded_utf8_end(text, 3, 1), 4);
}

#[rstest::rstest]
fn idempotent_mcp_requires_a_nonblank_idempotency_argument() {
	for argument in ["", " \t\n", "\u{2003}\u{00a0}"] {
		let config = json!({
			"transport":"mcp",
			"endpoint":"http://localhost:9999/mcp",
			"tool_name":"create",
			"replay":"idempotent",
			"idempotency_argument":argument
		});
		assert!(validate_config(&config).is_err(), "{argument:?}");
	}
	let valid = json!({
		"transport":"mcp",
		"endpoint":"http://localhost:9999/mcp",
		"tool_name":"create",
		"replay":"idempotent",
		"idempotency_argument":"request_id"
	});
	validate_config(&valid).unwrap();
}

#[rstest::rstest]
fn tool_transports_cannot_name_a_deployment_secret() {
	for config in [
		json!({"transport":"http","endpoint":"https://tools.example/rpc","credential_env":"AIDASH_SECRET_TEST_PEER","replay":"read_only"}),
		json!({"transport":"mcp","endpoint":"https://tools.example/mcp","credential_env":null,"tool_name":"lookup","replay":"read_only"}),
	] {
		let error = validate_config(&config).unwrap_err();
		assert!(
			error
				.to_string()
				.contains("cannot name a Deployment secret"),
			"{error}"
		);
	}
}
