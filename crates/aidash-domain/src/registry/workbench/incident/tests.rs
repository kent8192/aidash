use super::*;
use chrono::TimeZone;
use rstest::rstest;

fn evidence(title: &str, content: &str) -> EvidenceInput {
	EvidenceInput {
		title: title.into(),
		content: content.into(),
	}
}

#[rstest]
#[case("low", "open")]
#[case("medium", "resolved")]
#[case("high", "open")]
#[case("critical", "resolved")]
fn accepted_reporter_labels_remain_factual(#[case] severity: &str, #[case] status: &str) {
	assert!(validate(severity, status, "Report", &[]).is_ok());
}

#[rstest]
#[case("certified", "open", "Report")]
#[case("high", "closed", "Report")]
#[case("low", "open", "  ")]
fn invalid_labels_and_empty_notes_are_rejected(
	#[case] severity: &str,
	#[case] status: &str,
	#[case] notes: &str,
) {
	assert_eq!(
		validate(severity, status, notes, &[]),
		Err(crate::Error::Invalid(
			"invalid incident severity, status, notes or evidence".into()
		))
	);
}

#[rstest]
#[case(16_384, true)]
#[case(16_385, false)]
fn notes_preserve_the_byte_limit(#[case] bytes: usize, #[case] allowed: bool) {
	assert_eq!(
		validate("low", "open", &"a".repeat(bytes), &[]).is_ok(),
		allowed
	);
	assert!(validate("low", "open", &"あ".repeat(5_462), &[]).is_err());
}

#[rstest]
#[case(8, true)]
#[case(9, false)]
fn a_request_preserves_the_eight_copy_limit(#[case] copies: usize, #[case] allowed: bool) {
	let evidence = (0..copies)
		.map(|_| evidence("source", "fixed text"))
		.collect::<Vec<_>>();
	assert_eq!(
		validate("low", "open", "Report", &evidence).is_ok(),
		allowed
	);
}

#[rstest]
#[case(65_536, true)]
#[case(65_537, false)]
fn copied_payloads_preserve_the_aggregate_byte_limit(#[case] bytes: usize, #[case] allowed: bool) {
	let evidence = vec![
		evidence("first", &"a".repeat(32_768)),
		evidence("second", &"a".repeat(bytes - 32_768)),
	];
	assert_eq!(
		validate("low", "open", "Report", &evidence).is_ok(),
		allowed
	);
}

#[rstest]
#[case("", "content")]
#[case("source", " ")]
fn evidence_requires_nonempty_source_and_payload(#[case] title: &str, #[case] content: &str) {
	assert!(validate("low", "open", "Report", &[evidence(title, content)]).is_err());
}

#[rstest]
#[case(255, true)]
#[case(256, false)]
fn evidence_title_preserves_the_byte_limit(#[case] bytes: usize, #[case] allowed: bool) {
	assert_eq!(
		validate(
			"low",
			"open",
			"Report",
			&[evidence(&"a".repeat(bytes), "content")]
		)
		.is_ok(),
		allowed
	);
}

#[rstest]
fn copied_payload_keeps_exact_content_digest_and_supplied_retention_timestamp() {
	let recorded_at = Utc.timestamp_opt(100, 0).unwrap();
	let copy = fixed_copy(evidence("source", "hello world"), recorded_at);
	assert_eq!(copy.title, "source");
	assert_eq!(copy.content.as_deref(), Some("hello world"));
	assert_eq!(
		copy.sha256,
		"b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
	);
	assert_eq!(copy.recorded_at, recorded_at);
	let wire = serde_json::to_value(copy).unwrap();
	assert_eq!(wire["content"], "hello world");
	assert_eq!(wire.as_object().unwrap().len(), 4);
}
