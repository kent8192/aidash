use super::{bearer, browser_operator_allowed};
use http::Method;
use rstest::rstest;

#[rstest]
#[case::canonical("Bearer opaque-token", Some("opaque-token"))]
#[case::lowercase("bearer opaque-token", Some("opaque-token"))]
#[case::mixed_case("bEaReR opaque-token", Some("opaque-token"))]
#[case::token_case("BEARER CaseSensitive-Token", Some("CaseSensitive-Token"))]
#[case::different_scheme("Basic opaque-token", None)]
#[case::missing_separator("Bearer", None)]
fn bearer_schemes_preserve_case_sensitive_credentials(
	#[case] authorization: &str,
	#[case] expected: Option<&str>,
) {
	// Arrange: use the request header representation received by native endpoints.
	let request = reinhardt::Request::builder()
		.method(Method::GET)
		.uri("/api/session")
		.header("authorization", authorization)
		.build()
		.unwrap();
	// Act and Assert: only the scheme is case insensitive.
	assert_eq!(bearer(&request.headers), expected);
}

#[rstest]
#[case("/api/skills/import", true)]
#[case("/api/agents/personal", true)]
#[case("/api/agents", false)]
fn browser_operator_registry_creation(#[case] path: &str, #[case] permitted: bool) {
	assert_eq!(browser_operator_allowed(&Method::POST, path), permitted);
}

#[rstest]
#[case("/api/runs/run-id/management", true)]
#[case("/runs/run-id/management", true)]
#[case("/api/runs/run-id/control", true)]
#[case("/api/runs/run-id/resume", false)]
#[case("/api/runs/run-id/management/extra", false)]
#[case("/api/runs", false)]
fn browser_operator_run_writes_preserve_management_without_subject_actions(
	#[case] path: &str,
	#[case] permitted: bool,
) {
	let request = reinhardt::Request::builder()
		.method(Method::POST)
		.uri(path)
		.build()
		.unwrap();

	assert_eq!(
		browser_operator_allowed(&request.method, request.uri.path()),
		permitted
	);
}
