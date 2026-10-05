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
