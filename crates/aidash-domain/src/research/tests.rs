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
	let text = "alpha\nbeta\ngamma\n";
	assert_eq!(parts(text, 11), ["alpha\nbeta\n", "gamma\n"]);
	assert_eq!(parts("", 8), [""]);
	let long = "é".repeat(10);
	let split = parts(&long, 5);
	assert!(split.iter().all(|part| part.len() <= 5));
	assert_eq!(split.concat(), long);
	assert_eq!(parts(text, 1024), [text]);
}
