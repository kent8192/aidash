use super::*;
use rstest::rstest;

#[rstest]
#[case(Failure::Timeout, true)]
#[case(Failure::RateLimited, true)]
#[case(Failure::NoSuchPart, false)]
#[case(Failure::Refused, false)]
fn failures_are_typed_recoverable_results(#[case] failure: Failure, #[case] retryable: bool) {
	let result = super::failure(Operation::Fetch, failure);
	assert_eq!(result["operation"], "fetch");
	assert_eq!(result["status"], "error");
	assert_eq!(result["error"]["code"], failure.code());
	assert_eq!(result["error"]["retryable"], retryable);
	assert!(result.get("data").is_none());
}

#[test]
fn only_fetch_remains() {
	assert_eq!(Operation::parse("fetch"), Some(Operation::Fetch));
	for removed in ["search", "find", "view"] {
		assert_eq!(Operation::parse(removed), None);
	}
}

#[test]
fn parts_break_at_lines_and_never_exceed_the_bound() {
	// Each line break takes two bytes once encoded.
	let text = "alpha\nbeta\ngamma\n";
	assert_eq!(parts(text, 13), ["alpha\nbeta\n", "gamma\n"]);
	assert_eq!(parts("", 8), [""]);
	let long = "é".repeat(10);
	let split = parts(&long, 5);
	assert!(split.iter().all(|part| part.len() <= 5));
	assert_eq!(split.concat(), long);
	assert_eq!(parts(text, 1024), [text]);
}

#[rstest]
#[case::backslashes("\\".repeat(100))]
#[case::quotes("\"".repeat(100))]
#[case::tabs_and_short_lines("\t\n".repeat(50))]
#[case::other_control_characters("\u{1}".repeat(30))]
#[case::mixed("a\"b\\c\té\n".repeat(20))]
fn parts_are_sized_by_their_encoded_length(#[case] text: String) {
	let max = 64;
	let split = parts(&text, max);
	assert_eq!(split.concat(), text);
	for part in &split {
		assert!(!part.is_empty());
		let encoded = serde_json::to_string(part).unwrap().len() - 2;
		assert_eq!(escaped_len(part), encoded);
		assert!(encoded <= max, "{encoded} > {max}");
	}
}
