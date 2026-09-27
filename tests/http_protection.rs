mod common;

use aidash::{api, http::Settings};
use axum::{body::Body, http::Request};
use axum_test::TestServer;
use futures_util::StreamExt;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tower::ServiceExt;

#[rstest::rstest]
#[tokio::test]
async fn production_router_enforces_auth_validation_rate_and_sse_resume(
	#[future(awt)]
	#[from(common::test_environment)]
	environment: Arc<common::TestEnvironment>,
) {
	let (federation, url, schema) = common::setup(&environment).await;
	let limited = TestServer::new(api::router_with_settings(
		federation.clone(),
		Settings {
			actor_burst: 1,
			..Default::default()
		},
	))
	.unwrap();
	limited
		.get("/api/session")
		.await
		.assert_status_unauthorized();
	limited
		.get("/api/session")
		.authorization_bearer("operator-execution-fixture")
		.await
		.assert_status_ok();
	let limited_response = limited
		.get("/api/session")
		.authorization_bearer("operator-execution-fixture")
		.await;
	limited_response.assert_status_too_many_requests();
	assert!(limited_response.headers().contains_key("x-request-id"));
	// Exhausting the authenticated budget must not change unauthenticated errors.
	limited
		.get("/api/session")
		.await
		.assert_status_unauthorized();

	let app = api::router_with_settings(
		federation.clone(),
		Settings {
			sse_connections: 1,
			..Default::default()
		},
	);
	let server = TestServer::new(app.clone()).unwrap();
	let invalid = server
		.post("/api/workspaces")
		.authorization_bearer("operator-execution-fixture")
		.json(&json!({"title":"   ","goal":"valid"}))
		.await;
	invalid.assert_status_bad_request();
	assert_eq!(
		invalid.json::<serde_json::Value>()["error"],
		"title and goal must not be blank"
	);
	server
		.post("/api/workspaces")
		.authorization_bearer("operator-execution-fixture")
		.json(&json!({"title":"first","goal":"valid"}))
		.await
		.assert_status_ok();
	let before = federation
		.store
		.events(0, None, 500)
		.await
		.unwrap()
		.last()
		.unwrap()
		.sequence;
	server
		.post("/api/workspaces")
		.authorization_bearer("operator-execution-fixture")
		.json(&json!({"title":"second","goal":"valid"}))
		.await
		.assert_status_ok();
	let after = federation.store.events(before, None, 500).await.unwrap()[0].sequence;
	let stream_request = || {
		Request::builder()
			.uri("/api/events/stream?after=0")
			.header("authorization", "Bearer operator-execution-fixture")
			.header("last-event-id", before.to_string())
			.body(Body::empty())
			.unwrap()
	};
	let response = app.clone().oneshot(stream_request()).await.unwrap();
	assert_eq!(response.status(), 200);
	assert_eq!(
		app.clone()
			.oneshot(stream_request())
			.await
			.unwrap()
			.status(),
		503
	);
	let mut body = response.into_body().into_data_stream();
	let chunk = tokio::time::timeout(Duration::from_secs(5), body.next())
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	let text = std::str::from_utf8(&chunk).unwrap();
	assert!(text.contains(&format!("id: {after}\n")), "{text}");
	assert!(!text.contains(&format!("id: {before}\n")), "{text}");
	drop(body);
	assert_eq!(
		app.clone()
			.oneshot(stream_request())
			.await
			.unwrap()
			.status(),
		200
	);
	drop(server);
	drop(limited);
	drop(app);
	common::cleanup(federation, &url, &schema).await;
}
