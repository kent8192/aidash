use super::*;
use rstest::rstest;
use serde_json::json;
use std::collections::BTreeSet;

#[rstest]
#[case(
	"  The page owner asked us to remove it.\n",
	Some("The page owner asked us to remove it.")
)]
#[case("line one\nline two", Some("line one\nline two"))]
#[case("   ", None)]
#[case("bell\u{7}", None)]
fn a_revocation_needs_a_bounded_reason(#[case] input: &str, #[case] expected: Option<&str>) {
	assert_eq!(reason(input).ok().as_deref(), expected);
}

#[rstest]
fn a_reason_longer_than_the_bound_is_refused() {
	assert!(reason(&"a".repeat(MAX_REASON_CHARS)).is_ok());
	assert!(reason(&"a".repeat(MAX_REASON_CHARS + 1)).is_err());
}

#[rstest]
#[case(Operation::Search)]
#[case(Operation::Fetch)]
#[case(Operation::Find)]
fn tombstones_keep_the_operation_and_carry_no_content(#[case] operation: Operation) {
	let tombstone = tombstone(operation);
	assert_eq!(result_operation(&tombstone), Some(operation));
	assert_eq!(tombstone["error"]["code"], "evidence_revoked");
	assert!(tombstone.get("data").is_none());
}

#[rstest]
fn research_results_are_recognized_by_their_envelope() {
	let result = super::super::envelope(Operation::Fetch, "ok", json!({"text": "passage"}), false);
	assert_eq!(result_operation(&result), Some(Operation::Fetch));
	assert_eq!(result_operation(&json!({"operation": "fetch"})), None);
	assert_eq!(
		result_operation(&json!({"version": 1, "operation": "lookup", "limits": {}})),
		None
	);
	// Another tool's result with the same generic fields is not research.
	assert_eq!(
		result_operation(&json!({
			"version": 1,
			"operation": "search",
			"limits": {"max_bytes": 32768, "truncated": false},
		})),
		None
	);
}

fn tool(seq: u64, id: &str, result: Value) -> HistoryEntry {
	HistoryEntry {
		seq,
		event: ContextEvent::Tool {
			call: crate::provider::ToolCall {
				id: id.into(),
				name: "renamed_alias".into(),
				arguments: json!({}),
			},
			result,
		},
	}
}

#[rstest]
fn redaction_replaces_every_research_result_whatever_its_tool_alias() {
	let page = super::super::envelope(Operation::Fetch, "ok", json!({"text": "secret"}), false);
	let failed = super::super::failure(Operation::Search, Failure::SnapshotLimit);
	let other = json!({"status": "ok", "data": {"text": "workspace note"}});
	let cut = json!("{\"data\":{\"text\":\"sec[fast-jev-compaction truncated 9 chars]");
	let mut history = vec![
		tool(1, "read", page),
		tool(2, "search", failed),
		tool(3, "note", other.clone()),
		tool(4, "cut", cut),
	];
	assert!(redact(&mut history, &BTreeSet::from([4])));
	for (entry, id) in history.iter().zip(["read", "search", "note", "cut"]) {
		if id == "note" {
			continue;
		}
		let ContextEvent::Tool { result, .. } = &entry.event else {
			unreachable!()
		};
		assert_eq!(result, &ContextEvent::revoked_result(id));
		assert!(!carries_research(&entry.event));
	}
	let ContextEvent::Tool { result, .. } = &history[2].event else {
		unreachable!()
	};
	assert_eq!(result, &other);
	assert!(
		!redact(&mut history, &BTreeSet::from([4])),
		"a second pass changes nothing"
	);
}

#[rstest]
fn a_pending_approval_result_is_research_too() {
	let pending = json!({
		"contract": crate::research::RESULT_CONTRACT,
		"version": 1,
		"operation": "search",
		"status": "approval_required",
		"data": {"content": "a disclosed query"},
	});
	assert_eq!(result_operation(&pending), Some(Operation::Search));
}
