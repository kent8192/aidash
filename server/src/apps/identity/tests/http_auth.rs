use super::browser_operator_allowed;
use http::Method;
use rstest::rstest;

#[rstest]
#[case("/api/skills/import", true)]
#[case("/api/agents/personal", true)]
#[case("/api/agents", false)]
fn browser_operator_registry_creation(#[case] path: &str, #[case] permitted: bool) {
	assert_eq!(browser_operator_allowed(&Method::POST, path), permitted);
}
