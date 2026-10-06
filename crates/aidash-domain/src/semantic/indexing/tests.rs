use super::*;
use rstest::rstest;

#[rstest]
#[case::blank("  ", 32, false)]
#[case::empty("", 32, false)]
#[case::exact("abc", 3, true)]
#[case::overflow("abcd", 3, false)]
#[case::utf8("é", 1, false)]
#[case::utf8_exact("é", 2, true)]
fn indexing_text_limits_are_utf8_bytes(
	#[case] text: &str,
	#[case] max: usize,
	#[case] allowed: bool,
) {
	assert_eq!(validate_text(text, max).is_ok(), allowed);
}
#[rstest]
#[case::first(0, 1, 2.0)]
#[case::next(1, 2, 4.0)]
#[case::bounded(8, 9, 256.0)]
#[case::saturated(i32::MAX, i32::MAX, 256.0)]
fn retries_keep_the_original_saturating_attempt_and_exponential_delay(
	#[case] attempts: i32,
	#[case] next: i32,
	#[case] delay: f64,
) {
	assert_eq!(retry(attempts), (next, delay));
}
#[rstest]
fn content_hash_preserves_the_persisted_hex_format() {
	assert_eq!(
		content_digest("abc"),
		"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
	);
}
#[rstest]
#[case::memory(r#"{"kind":"memory","text":"bytes"}"#)]
#[case::artifact(r#"{"kind":"artifact","id":"00000000-0000-0000-0000-000000000001"}"#)]
#[case::message(r#"{"kind":"message","id":"00000000-0000-0000-0000-000000000001"}"#)]
fn source_contracts_round_trip_without_added_fields(#[case] encoded: &str) {
	let value: Value = serde_json::from_str(encoded).unwrap();
	let source: Source = serde_json::from_value(value.clone()).unwrap();
	assert_eq!(serde_json::to_value(source).unwrap(), value);
}
