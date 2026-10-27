use super::{bearer, browser_deployment_administrator_allowed};
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
#[case::invitation("/api/tenants/t/invitations", true)]
#[case::invitation_revoke("/api/tenants/t/invitations/i/revoke", true)]
#[case::registration("/api/tenants/t/registrations/r/approve", true)]
#[case::deletion("/api/tenants/t/deletion", true)]
#[case::policy("/api/authorization/t", true)]
#[case::skills_import("/api/skills/import", false)]
#[case::personal_agents("/api/agents/personal", false)]
#[case::provider_credentials("/api/tenants/t/provider-credentials", false)]
#[case::bindings("/api/tenants/t/provider-credential-bindings/b", false)]
#[case::retention("/api/tenants/t/retention", false)]
#[case::tasks_abandon("/api/tasks/x/abandon", false)]
#[case::research_revocation("/api/runs/r/research-revocation", false)]
#[case::workbench("/api/workbench/drafts", false)]
#[case::registry("/api/registry", false)]
#[case::transactions("/api/transactions", false)]
#[case::remote("/api/remote", false)]
fn browser_deployment_administrator_writes_only_on_administered_routes(
	#[case] path: &str,
	#[case] permitted: bool,
) {
	assert_eq!(
		browser_deployment_administrator_allowed(&Method::POST, path),
		permitted
	);
}

#[rstest]
#[case("/api/runs/run-id/management", true)]
#[case("/runs/run-id/management", true)]
#[case("/api/runs/run-id/control", true)]
#[case("/api/runs/run-id/resume", false)]
#[case("/api/runs/run-id/management/extra", false)]
#[case("/api/runs", false)]
fn browser_deployment_administrator_run_writes_preserve_management_without_subject_actions(
	#[case] path: &str,
	#[case] permitted: bool,
) {
	let request = reinhardt::Request::builder()
		.method(Method::POST)
		.uri(path)
		.build()
		.unwrap();

	assert_eq!(
		browser_deployment_administrator_allowed(&request.method, request.uri.path()),
		permitted
	);
}
