use super::*;
use crate::research::envelope;
use rstest::{fixture, rstest};
use serde_json::json;

const SEARCHED: &str = "0190f1d2-0000-7000-8000-000000000001";
const READ: &str = "0190f1d2-0000-7000-8000-000000000002";
const OTHER: &str = "0190f1d2-0000-7000-8000-000000000003";

#[fixture]
fn expired() -> ExpiredResults {
	ExpiredResults {
		sources: [SEARCHED.parse().unwrap()].into(),
		documents: [READ.parse().unwrap()].into(),
		through: None,
	}
}

fn tool(seq: u64, id: &str, result: Value) -> HistoryEntry {
	HistoryEntry {
		seq,
		event: ContextEvent::Tool {
			call: crate::provider::ToolCall {
				id: id.into(),
				name: "web_search".into(),
				arguments: json!({"query": "what Aidash sent"}),
			},
			result,
		},
	}
}

fn search(source: &str) -> Value {
	envelope(
		Operation::Search,
		"ok",
		json!({"sources": [{"source_id": source, "title": "t", "snippet": "s"}]}),
		false,
	)
}

fn read(operation: Operation, source: &str, document: &str) -> Value {
	envelope(
		operation,
		"ok",
		json!({"source_id": source, "document_id": document, "text": "passage"}),
		false,
	)
}

#[rstest]
#[case::search_of_an_expired_source(search(SEARCHED), true)]
#[case::search_of_another_source(search(OTHER), false)]
#[case::read_of_an_expired_snapshot(read(Operation::Fetch, OTHER, READ), true)]
#[case::find_in_an_expired_snapshot(read(Operation::Find, OTHER, READ), true)]
// Aidash's own page reader read a searched Source: not a Provider result.
#[case::native_read_of_a_searched_source(read(Operation::Fetch, SEARCHED, OTHER), false)]
#[case::not_research(json!({"data": {"document_id": READ}}), false)]
fn only_results_built_from_expired_provider_results_are_covered(
	expired: ExpiredResults,
	#[case] result: Value,
	#[case] covered: bool,
) {
	assert_eq!(expired.covers(&result), covered);
}

#[rstest]
fn expiry_replaces_results_but_keeps_what_aidash_sent(expired: ExpiredResults) {
	let revoked = ContextEvent::revoked_result("revoked");
	let mut history = vec![
		tool(1, "searched", search(SEARCHED)),
		tool(2, "kept", search(OTHER)),
		tool(
			3,
			"cut",
			json!("{\"data\":{\"sources\":[{\"title\":\"t[truncated]"),
		),
		tool(4, "revoked", revoked.clone()),
	];
	assert!(expire(&mut history, &expired, &[3].into()));
	let results = history
		.iter()
		.map(|entry| match &entry.event {
			ContextEvent::Tool { call, result } => {
				assert_eq!(call.arguments, json!({"query": "what Aidash sent"}));
				result.clone()
			}
			_ => unreachable!(),
		})
		.collect::<Vec<_>>();
	assert_eq!(results[0], ContextEvent::expired_result("searched"));
	assert_eq!(results[1], search(OTHER));
	assert_eq!(results[2], ContextEvent::expired_result("cut"));
	assert_eq!(results[3], revoked);
	// Applying it again changes nothing.
	assert!(!expire(&mut history, &expired, &[3].into()));
}

#[rstest]
fn tombstones_keep_the_operation_and_carry_no_content() {
	let tombstone = tombstone(Operation::Search);
	assert_eq!(result_operation(&tombstone), Some(Operation::Search));
	assert_eq!(tombstone["error"]["code"], "result_expired");
	assert!(tombstone.get("data").is_none());
	assert!(!ExpiredResults::default().covers(&tombstone));
}

#[test]
fn an_expired_placeholder_still_loses_its_disclosed_arguments_on_revocation() {
	let mut history = vec![tool(1, "expired", ContextEvent::expired_result("expired"))];
	assert!(crate::research::revocation::redact(
		&mut history,
		&Default::default()
	));
	let ContextEvent::Tool { call, result } = &history[0].event else {
		panic!("tool expected")
	};
	assert_eq!(call.arguments, ContextEvent::revoked_arguments());
	assert!(ContextEvent::is_revoked_result(result));
}
