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
			"credential_env":null,
			"tool_name":"create",
			"replay":"idempotent",
			"idempotency_argument":argument
		});
		assert!(validate_config_in(&config, false).is_err(), "{argument:?}");
	}
	let valid = json!({
		"transport":"mcp",
		"endpoint":"http://localhost:9999/mcp",
		"credential_env":null,
		"tool_name":"create",
		"replay":"idempotent",
		"idempotency_argument":"request_id"
	});
	validate_config_in(&valid, false).unwrap();
}
