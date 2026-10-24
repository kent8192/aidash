use super::*;
use rstest::rstest;

#[rstest]
#[case(Failure::Timeout, "uncertain", false)]
#[case(Failure::OutcomeUnknown, "uncertain", false)]
#[case(Failure::RateLimited, "error", true)]
#[case(Failure::DisclosureDenied, "error", false)]
fn failures_are_typed_recoverable_results(
	#[case] failure: Failure,
	#[case] status: &str,
	#[case] retryable: bool,
) {
	let result = super::failure(Operation::Search, failure);
	assert_eq!(result["operation"], "search");
	assert_eq!(result["status"], status);
	assert_eq!(result["error"]["code"], failure.code());
	assert_eq!(result["error"]["retryable"], retryable);
	assert!(result.get("data").is_none());
}

#[rstest]
#[case("search", Some(Operation::Search), true)]
#[case("fetch", Some(Operation::Fetch), true)]
#[case("find", Some(Operation::Find), false)]
#[case("lookup", None, false)]
fn operations_and_disclosure_are_fixed_by_the_contract(
	#[case] name: &str,
	#[case] operation: Option<Operation>,
	#[case] discloses: bool,
) {
	assert_eq!(Operation::parse(name), operation);
	if let Some(operation) = operation {
		assert_eq!(operation.as_str(), name);
		assert_eq!(operation.discloses(), discloses);
	}
}

#[rstest]
fn bounded_text_keeps_a_character_boundary() {
	assert_eq!(bounded("長長", 4), ("長", true));
	assert_eq!(bounded("ab", 2), ("ab", false));
}
