use super::*;
use rstest::rstest;
#[rstest]
#[case::claim("claim")]
#[case::transition("transition")]
#[case::terminal("run_message_terminal_transition")]
#[case::complete("run_message_complete")]
#[case::artifact("artifact")]
#[case::create("create_task")]
#[case::delegate("delegate")]
#[case::message("message")]
#[case::output("run_message_output")]
#[case::event("event")]
fn effect_commands_use_the_durable_replay_journal(#[case] operation: &str) {
	assert!(prepare(operation, &json!({"key":"id"})).unwrap().mutation);
}
#[rstest]
#[case::task("task")]
#[case::snapshot("snapshot")]
#[case::record("workspace_record")]
#[case::chunk("workspace_record_chunk")]
#[case::children("workspace_children")]
#[case::unknown("unknown")]
fn reads_and_unknown_commands_do_not_enter_the_mutation_journal(#[case] operation: &str) {
	assert!(!prepare(operation, &json!({})).unwrap().mutation);
}
#[rstest]
#[case::absent(json!({}),"transition:null")]
#[case::revision(json!({"revision":7}),"transition:7")]
#[case::named(json!({"key":"saved","revision":7}),"transition:saved")]
fn replay_keys_retain_the_original_operation_and_revision_namespace(
	#[case] data: Value,
	#[case] expected: &str,
) {
	assert_eq!(prepare("transition", &data).unwrap().request_key, expected);
}
#[rstest]
#[case::empty(json!({"key":""}),"invalid command key")]
#[case::overlong(json!({"key":"a".repeat(513)}),"invalid command key")]
fn named_keys_keep_the_existing_byte_bound(#[case] data: Value, #[case] message: &str) {
	assert!(matches!(prepare("message",&data),Err(Error::Invalid(saved)) if saved==message));
}
#[rstest]
#[case::missing(json!({}))]
#[case::non_string(json!({"key":3}))]
fn delegation_requires_an_explicit_string_key(#[case] data: Value) {
	assert!(
		matches!(prepare("delegate",&data),Err(Error::Invalid(message)) if message=="missing command key")
	);
}
#[rstest]
fn metadata_digest_retains_the_original_serialized_input() {
	let metadata = prepare("message", &json!({"key":"id","content":"saved"})).unwrap();
	assert_eq!(
		metadata.digest,
		"sha256:732e5dd0fa199cd7109a48cacc901b120ca35324c17242ad9d13419e80183303"
	);
	assert_ne!(
		metadata.digest,
		prepare("message", &json!({"key":"id","content":"changed"}))
			.unwrap()
			.digest
	);
}
#[rstest]
#[case::human("human:", true)]
#[case::subject("subject-human:tenant:", true)]
#[case::wrong("human:other:", false)]
fn scoped_input_keys_bind_to_the_current_run(#[case] prefix: &str, #[case] permitted: bool) {
	let run = Uuid::from_u128(1);
	let key = if prefix == "human:other:" {
		format!("{prefix}key")
	} else {
		format!("{prefix}{run}:key")
	};
	assert_eq!(require_input_key(&key, run).is_ok(), permitted);
}

#[test]
fn personal_connection_calls_require_their_logical_key_and_listing_is_a_read() {
	assert!(prepare("mcp.connection_call", &json!({"key":"unrelated"})).is_err());
	assert!(prepare("mcp.connection_call", &json!({"idempotency_key":""})).is_err());
	let metadata = prepare(
		"mcp.connection_call",
		&json!({"idempotency_key":"logical-call"}),
	)
	.unwrap();
	assert!(metadata.mutation);
	assert_eq!(metadata.request_key, "mcp.connection_call:logical-call");
	assert!(!prepare("mcp.connection_list", &json!({})).unwrap().mutation);
}

#[test]
fn a_tools_own_fallback_shaped_result_never_becomes_a_control_outcome() {
	use super::connections::{Outcome, retryable};
	for literal in [
		json!({"outcome":"not_personal"}),
		json!({"error":"connection_required","resource":"https://mcp.example/tools"}),
		Value::Null,
	] {
		let wire = json!({"outcome":"result","result":literal});
		assert!(!retryable(&wire));
		let received: Outcome = serde_json::from_value(wire).unwrap();
		assert!(matches!(received, Outcome::Result { result } if result == literal));
	}
	assert!(retryable(&json!({"outcome":"not_personal"})));
	assert!(retryable(
		&json!({"outcome":"connection","cause":{"error":"connection_required","resource":"https://mcp.example/tools"}})
	));
}
