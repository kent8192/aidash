use super::*;
use rstest::rstest;
#[rstest]
#[case("active", true)]
#[case("running", true)]
#[case("deleted", false)]
#[case("recoverable", false)]
#[case("uncertain", false)]
#[case("cleaning", false)]
#[case("cleanup_failed", false)]
#[case("retained", false)]
fn retained_context_cannot_be_reused(#[case] state: &str, #[case] expected: bool) {
	assert_eq!(reusable_context(state), expected);
}
#[rstest]
#[case(vec!["alice","parent","agent"],"alice","agent",true)]
#[case(vec!["mallory","agent"],"alice","agent",false)]
#[case(vec!["alice","other"],"alice","agent",false)]
#[case(vec![],"alice","agent",false)]
fn disclosure_requires_both_delegation_endpoints(
	#[case] subjects: Vec<&str>,
	#[case] owner: &str,
	#[case] agent: &str,
	#[case] expected: bool,
) {
	assert_eq!(
		received_scope_matches(
			&subjects.into_iter().map(str::to_owned).collect::<Vec<_>>(),
			owner,
			agent
		),
		expected
	);
}
