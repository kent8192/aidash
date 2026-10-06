use crate::endpoint::{EndpointFixture, assert_json, endpoint, subject};
use reinhardt::test::fixtures::api_client_from_url;
use rstest::rstest;

#[rstest]
#[case::canonical("Bearer")]
#[case::lowercase("bearer")]
#[case::mixed_case("bEaReR")]
#[tokio::test]
async fn bearer_authentication_is_request_scoped_and_operator_routes_reject_subjects(
	#[future] endpoint: EndpointFixture,
	#[case] scheme: &str,
) {
	// Arrange
	let app = endpoint.await;
	app.operator
		.set_header(
			"Authorization",
			&format!("{scheme} {}", app.runtime.config.api_token),
		)
		.await
		.unwrap();
	let alice = subject(&app, "alice").await;
	let invalid = api_client_from_url(&app.server.url);
	invalid
		.set_header("Authorization", "Bearer invalid-credential")
		.await
		.unwrap();
	// Act
	let operator = app.operator.get("/api/session").await.unwrap();
	let user = alice.get("/api/session").await.unwrap();
	let denied = alice.get("/api/transactions/trust").await.unwrap();
	let transactions = alice.get("/api/transactions").await.unwrap();
	let missing = app.anonymous.get("/api/session").await.unwrap();
	let invalid = invalid.get("/api/session").await.unwrap();
	// Assert
	assert_eq!(operator.header("cache-control"), Some("no-store"));
	assert_eq!(assert_json(operator, 200)["access"]["kind"], "operator");
	assert_eq!(assert_json(user, 200)["access"]["subject"], "alice");
	assert_json(denied, 403);
	assert_eq!(assert_json(transactions, 200), serde_json::json!([]));
	assert_json(missing, 401);
	assert_json(invalid, 401);
}
