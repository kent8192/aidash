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
			actor_period: Duration::from_secs(60),
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
	assert_eq!(limited_response.headers()["cache-control"], "no-store");
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
	let server = TestServer::new(
		app.clone()
			.layer(axum::Extension(axum::extract::ConnectInfo(
				"127.0.0.1:12345".parse::<std::net::SocketAddr>().unwrap(),
			))),
	)
	.unwrap();
	for (path, status) in [("/api/session", 401), ("/auth/config", 200)] {
		let response = server.get(path).await;
		assert_eq!(response.status_code().as_u16(), status);
		assert_eq!(response.headers()["cache-control"], "no-store");
		assert_eq!(response.headers()["referrer-policy"], "no-referrer");
	}
	let peer_denied = server
		.post("/federation/v0.1/discover")
		.add_header("x-aidash-protocol", aidash::config::PROTOCOL_VERSION)
		.json(&json!({}))
		.await;
	peer_denied.assert_status_unauthorized();
	assert_eq!(peer_denied.headers()["cache-control"], "no-store");
	assert_eq!(peer_denied.headers()["referrer-policy"], "no-referrer");
	let public = server.get("/api/openapi.json").await;
	public.assert_status_ok();
	assert!(!public.headers().contains_key("cache-control"));
	let static_response = server.get("/assets/missing.js").await;
	assert!(!static_response.headers().contains_key("cache-control"));
	let session = server
		.get("/api/session")
		.authorization_bearer("operator-execution-fixture")
		.await;
	session.assert_status_ok();
	assert_eq!(session.headers()["cache-control"], "no-store");
	assert_eq!(session.headers()["referrer-policy"], "no-referrer");
	// Route composition must preserve the default limit and the larger file
	// payload limit, including rejections that happen before extraction.
	for (method, path, limit) in [
		("GET", "/health", 1 << 20),
		("GET", "/assets/missing.js", 1 << 20),
		("GET", "/api/openapi.json", 1 << 20),
		("GET", "/auth/config", 1 << 20),
		("GET", "/api/dashboard/registrations", 1 << 20),
		("POST", "/api/transactions", 1 << 20),
		("POST", "/api/workspaces", 1 << 20),
		("POST", "/api/references/uploads", 6 << 20),
	] {
		let response = server
			.method(method.parse().unwrap(), path)
			.authorization_bearer("operator-execution-fixture")
			.add_header(axum::http::header::CONTENT_LENGTH, (limit + 1).to_string())
			.bytes(vec![b' '; limit + 1].into())
			.await;
		assert_eq!(response.status_code(), 413, "{method} {path}");
		assert!(response.headers().contains_key("x-request-id"));
	}
	for (path, limit) in [
		("/api/workspaces", 1 << 20),
		("/api/references/uploads", 6 << 20),
	] {
		let body = Body::from_stream(futures_util::stream::iter([
			Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![b' '; limit])),
			Ok(axum::body::Bytes::from_static(b"{}")),
		]));
		let response = app
			.clone()
			.oneshot(
				Request::builder()
					.method("POST")
					.uri(path)
					.header("authorization", "Bearer operator-execution-fixture")
					.header("content-type", "application/json")
					.body(body)
					.unwrap(),
			)
			.await
			.unwrap();
		assert_eq!(response.status(), 413, "{path}");
		assert!(response.headers().contains_key("x-request-id"));
	}
	let invalid_conversation = server.post("/api/conversations").authorization_bearer("operator-execution-fixture")
        .json(&json!({"title":" ", "goal":" ", "target":{"id":"missing", "version":"1.0.0"}, "target_kind":"agent"})).await;
	invalid_conversation.assert_status_bad_request();
	assert_eq!(
		invalid_conversation.json::<serde_json::Value>()["error"],
		"invalid request fields: goal, title"
	);
	for (body, content_type, status) in [
		("{", "application/json", 400),
		("{}", "application/json", 422),
		("{}", "text/plain", 415),
	] {
		let response = server
			.post("/api/workspaces")
			.authorization_bearer("operator-execution-fixture")
			.bytes(body.into())
			.content_type(content_type)
			.await;
		assert_eq!(response.status_code().as_u16(), status);
		assert_eq!(
			response.json::<serde_json::Value>()["error"],
			"invalid JSON request"
		);
	}

	let invalid = server
		.post("/api/workspaces")
		.authorization_bearer("operator-execution-fixture")
		.json(&json!({"title":"   ","goal":"valid"}))
		.await;
	invalid.assert_status_bad_request();
	assert_eq!(
		invalid.json::<serde_json::Value>()["error"],
		"invalid request fields: title"
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

#[tokio::test]
#[should_panic(expected = "Deserializing response from Json")]
async fn json_test_helper_does_not_hide_malformed_responses() {
	let app = axum::Router::new().route("/broken", axum::routing::get(|| async { "not JSON" }));
	common::request(&app, "fixture", "GET", "/broken", serde_json::Value::Null).await;
}
