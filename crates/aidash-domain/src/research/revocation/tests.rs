use super::*;
use rstest::rstest;

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
}
