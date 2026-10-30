//! A Deployment Administrator holds no Tenant authority (#484, #301): the
//! shared bearer is refused on Tenant content routes before the handler runs,
//! and a Subject Credential issued from the host reaches the same routes.
use crate::endpoint::{EndpointFixture, endpoint, subject_credential, tenant_scope};
use rstest::rstest;

#[rstest]
#[tokio::test]
async fn shared_bearer_is_refused_on_tenant_content_but_not_on_administered_routes(
	#[future] endpoint: EndpointFixture,
) {
	let app = endpoint.await;
	tenant_scope(&app.runtime.store, "acme").await;
	let token = subject_credential(&app.runtime.store, "acme", "alice").await;
	let content = "/api/tasks";

	// The shared bearer reaches no Tenant content, and the refusal carries none.
	let refused = app.deployment_administrator.get(content).await.unwrap();
	assert_eq!(refused.status_code(), 403, "{}", refused.text());
	assert!(!refused.text().contains("\"id\""));
	let refused = app
		.deployment_administrator
		.get("/api/tasks")
		.await
		.unwrap();
	assert_eq!(refused.status_code(), 403, "{}", refused.text());

	// The Subject Credential issued from the host reads the same route.
	let served = app
		.anonymous
		.request("GET".parse().unwrap(), content)
		.header("Authorization", format!("Bearer {token}"))
		.send()
		.await
		.unwrap();
	assert_eq!(served.status_code(), 200, "{}", served.text());

	// An administered route still admits the shared bearer.
	let invitations = app
		.deployment_administrator
		.get("/api/tenants/acme/invitations")
		.await
		.unwrap();
	assert_eq!(invitations.status_code(), 200, "{}", invitations.text());

	// `/api/state` carries the access profile and no Tenant collections.
	let state = app
		.deployment_administrator
		.get("/api/state")
		.await
		.unwrap();
	assert_eq!(state.status_code(), 200, "{}", state.text());
	let state = state.json_value().unwrap();
	for collection in [
		"workspaces",
		"tasks",
		"runs",
		"events",
		"artifacts",
		"conversations",
	] {
		assert_eq!(state[collection], serde_json::json!([]), "{collection}");
	}
}
