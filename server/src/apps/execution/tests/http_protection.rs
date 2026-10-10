#[path = "support/legacy.rs"]
mod common;

use aidash_server::http::Settings;
use bytes::Bytes;
use futures_util::StreamExt;
use http::Request;
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[rstest::rstest]
#[tokio::test]
async fn production_router_enforces_auth_validation_rate_and_sse_resume(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
	#[from(protection_settings)] _state: ProtectionSettings,
	#[future(awt)]
	#[from(common::native_application)]
	#[with(_state._auth_settings.clone(), aidash_server::sse::Service::new(aidash_server::sse::Settings::default()), Arc::new(|router| router), runtime.clone())]
	auth_fixture: common::ApplicationFixture,
	#[future(awt)]
	#[from(common::native_application)]
	#[with(_state._actor_settings.clone(), aidash_server::sse::Service::new(aidash_server::sse::Settings::default()), Arc::new(|router| router), runtime.clone())]
	actor_fixture: common::ApplicationFixture,
	#[future(awt)]
	#[from(common::native_application)]
	#[with(_state._stream_settings.clone(), aidash_server::sse::Service::new(aidash_server::sse::Settings::default()), Arc::new(|router| router), runtime.clone())]
	stream_fixture: common::ApplicationFixture,
	#[from(reinhardt::test::fixtures::http_client)] http_client: reqwest::Client,
) {
	let ProtectionSettings {
		_auth_settings,
		_actor_settings,
		_stream_settings,
	} = _state;
	let (federation, url, schema) = runtime.await.parts();
	let auth = auth_fixture.application;
	// Synthetic socket peers exercise trusted-proxy policy through the native handler.
	for (peer, client, expected) in [
		("127.0.0.1:1", "198.51.100.1", 200),
		("127.0.0.1:2", "198.51.100.1", 429),
		("127.0.0.1:3", "198.51.100.2", 200),
		("[::1]:1", "2001:db8::1", 200),
		("[::1]:2", "2001:db8::1", 429),
		("[::1]:3", "2001:db8::2", 200),
		("10.0.0.1:1", "198.51.100.3", 200),
		("10.0.0.1:2", "198.51.100.4", 429),
		("127.0.0.1:4", "198.51.100.5, 198.51.100.6", 200),
		("127.0.0.1:5", "invalid", 429),
	] {
		let response = auth
			.clone()
			.native_oneshot(
				Request::builder()
					.uri("/auth/config")
					.extension(peer.parse::<std::net::SocketAddr>().unwrap())
					.header("x-real-ip", client)
					.body(Bytes::new())
					.unwrap(),
			)
			.await
			.unwrap();
		assert_eq!(
			response.status.as_u16(),
			expected,
			"peer={peer}, client={client}"
		);
	}
	let repeated_header = auth
		.clone()
		.native_oneshot(
			Request::builder()
				.uri("/auth/config")
				.extension("127.0.0.1:6".parse::<std::net::SocketAddr>().unwrap())
				.header("x-real-ip", "198.51.100.7")
				.header("x-real-ip", "198.51.100.8")
				.body(Bytes::new())
				.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(
		repeated_header.status, 429,
		"repeated headers must use the exhausted peer budget"
	);
	drop(auth);

	let limited_app = actor_fixture.application;
	let limited = actor_fixture.operator;
	let limited_anonymous = actor_fixture.anonymous;
	assert_eq!(
		limited_anonymous
			.get("/api/session")
			.await
			.unwrap()
			.status_code(),
		401
	);
	assert_eq!(
		limited.get("/api/session").await.unwrap().status_code(),
		200
	);
	let limited_response = limited.get("/api/session").await.unwrap();
	assert_eq!(limited_response.status_code(), 429);
	assert!(limited_response.headers().contains_key("x-request-id"));
	assert_eq!(limited_response.headers()["cache-control"], "no-store");
	// Exhausting the authenticated budget must not change unauthenticated errors.
	assert_eq!(
		limited_anonymous
			.get("/api/session")
			.await
			.unwrap()
			.status_code(),
		401
	);

	let app = stream_fixture.application;
	let server = stream_fixture.anonymous;
	let operator = stream_fixture.operator;
	for (path, status) in [("/api/session", 401), ("/auth/config", 200)] {
		let response = server.get(path).await.unwrap();
		assert_eq!(response.status_code(), status);
		assert_eq!(response.headers()["cache-control"], "no-store");
		assert_eq!(response.headers()["referrer-policy"], "no-referrer");
	}
	let peer_denied = async {
		let client = &(server);
		let mut request = client
			.request(http::Method::POST, "/federation/v0.1/discover")
			.body(bytes::Bytes::copy_from_slice(b"{}"))
			.header(http::header::CONTENT_TYPE, "application/json");
		request = request.header("x-aidash-protocol", aidash_server::config::PROTOCOL_VERSION);
		request.send().await
	}
	.await
	.unwrap();
	assert_eq!(peer_denied.status_code(), 401);
	assert_eq!(peer_denied.headers()["cache-control"], "no-store");
	assert_eq!(peer_denied.headers()["referrer-policy"], "no-referrer");
	let public = server.get("/api/openapi.json").await.unwrap();
	assert_eq!(public.status_code(), 200);
	assert!(!public.headers().contains_key("cache-control"));
	let static_response = server.get("/assets/missing.js").await.unwrap();
	assert!(!static_response.headers().contains_key("cache-control"));
	let session = operator.get("/api/session").await.unwrap();
	assert_eq!(session.status_code(), 200);
	assert_eq!(session.headers()["cache-control"], "no-store");
	assert_eq!(session.headers()["referrer-policy"], "no-referrer");
	// APIClient buffers request bodies. The raw HTTP
	// fixture checks transport limits before extraction for both encodings.
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
		let response = http_client
			.request(method.parse().unwrap(), app.url(path))
			.bearer_auth("operator-execution-fixture")
			.header(http::header::CONTENT_LENGTH, (limit + 1).to_string())
			.body(vec![b' '; limit + 1])
			.send()
			.await
			.unwrap();
		assert_eq!(response.status(), 413, "{method} {path}");
		assert!(response.headers().contains_key("x-request-id"));
	}
	for (path, limit) in [
		("/api/workspaces", 1 << 20),
		("/api/references/uploads", 6 << 20),
	] {
		let body = reqwest::Body::wrap_stream(futures_util::stream::iter([
			Ok::<_, std::io::Error>(Bytes::from(vec![b' '; limit])),
			Ok(Bytes::from_static(b"{}")),
		]));
		let response = http_client
			.post(app.url(path))
			.bearer_auth("operator-execution-fixture")
			.header("content-type", "application/json")
			.body(body)
			.send()
			.await
			.unwrap();
		assert_eq!(response.status(), 413, "{path}");
		assert!(response.headers().contains_key("x-request-id"));
	}
	let invalid_conversation = operator.post("/api/conversations", &json!({"title":" ", "goal":" ", "target":{"id":"missing", "version":"1.0.0"}, "target_kind":"agent"}), "json").await.unwrap();
	assert_eq!(invalid_conversation.status_code(), 400);
	assert_eq!(
		invalid_conversation.json_value().unwrap()["error"],
		"invalid request fields: goal, title"
	);
	for (body, content_type, status) in [
		("{", "application/json", 400),
		("{}", "application/json", 422),
		("{}", "text/plain", 415),
	] {
		let response = operator
			.post_raw("/api/workspaces", body.as_bytes(), content_type)
			.await
			.unwrap();
		assert_eq!(response.status_code(), status);
		assert_eq!(
			response.json_value().unwrap()["error"],
			"invalid JSON request"
		);
	}
	let invalid = operator
		.post(
			"/api/workspaces",
			&json!({"title":"   ","goal":"valid"}),
			"json",
		)
		.await
		.unwrap();
	assert_eq!(invalid.status_code(), 400);
	assert_eq!(
		invalid.json_value().unwrap()["error"],
		"invalid request fields: title"
	);
	assert_eq!(
		operator
			.post(
				"/api/workspaces",
				&json!({"title":"first","goal":"valid"}),
				"json"
			)
			.await
			.unwrap()
			.status_code(),
		200
	);
	let before = federation
		.store
		.events(0, None, 500)
		.await
		.unwrap()
		.last()
		.unwrap()
		.sequence;
	assert_eq!(
		operator
			.post(
				"/api/workspaces",
				&json!({"title":"second","goal":"valid"}),
				"json"
			)
			.await
			.unwrap()
			.status_code(),
		200
	);
	let after = federation.store.events(before, None, 500).await.unwrap()[0].sequence;
	let stream_request = || {
		Request::builder()
			.uri("/api/events/stream?after=0")
			.header("authorization", "Bearer operator-execution-fixture")
			.header("last-event-id", before.to_string())
			.body(Bytes::new())
			.unwrap()
	};
	// Retain the unpolled response to verify stream admission and producer ownership.
	let mut response = app.clone().native_oneshot(stream_request()).await.unwrap();
	assert_eq!(response.status, 200);
	assert_eq!(
		app.clone()
			.native_oneshot(stream_request())
			.await
			.unwrap()
			.status,
		503
	);
	let mut body = response.take_stream_body().unwrap();
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
			.native_oneshot(stream_request())
			.await
			.unwrap()
			.status,
		200
	);
	drop(server);
	drop(limited);
	drop(app);
	drop(limited_app);
	common::cleanup(federation, &url, &schema).await;
}
struct MalformedJson;

#[async_trait::async_trait]
impl reinhardt::Handler for MalformedJson {
	async fn handle(&self, _request: reinhardt::Request) -> reinhardt::Result<reinhardt::Response> {
		Ok(reinhardt::Response::ok().with_body(b"not JSON".to_vec()))
	}
}

#[rstest::rstest]
#[tokio::test]
#[should_panic(expected = "JSON endpoint response")]
async fn json_test_helper_does_not_hide_malformed_responses(
	#[from(broken_routes)] _routes: common::RouterTransform,
	#[future(awt)]
	#[from(common::native_application)]
	#[with(Settings::default(), aidash_server::sse::Service::new(aidash_server::sse::Settings::default()), _routes.clone())]
	fixture: common::ApplicationFixture,
) {
	let app = fixture.application;
	common::request(&app, "fixture", "GET", "/broken", serde_json::Value::Null).await;
}

#[rstest::fixture]
fn auth_settings() -> Settings {
	Settings {
		auth_burst: 1,
		auth_period: Duration::from_secs(60),
		auth_trusted_proxy_ips: vec!["127.0.0.1".parse().unwrap(), "::1".parse().unwrap()],
		..Default::default()
	}
}
#[rstest::fixture]
fn actor_settings() -> Settings {
	Settings {
		actor_burst: 1,
		actor_period: Duration::from_secs(60),
		..Default::default()
	}
}
#[rstest::fixture]
fn stream_settings() -> Settings {
	Settings {
		sse_connections: 1,
		..Default::default()
	}
}
#[rstest::fixture]
fn broken_routes() -> common::RouterTransform {
	Arc::new(|router| router.handler("/broken", MalformedJson))
}

#[derive(Clone)]
struct ProtectionSettings {
	_auth_settings: Settings,
	_actor_settings: Settings,
	_stream_settings: Settings,
}
#[rstest::fixture]
fn protection_settings(
	#[from(auth_settings)] _auth_settings: Settings,
	#[from(actor_settings)] _actor_settings: Settings,
	#[from(stream_settings)] _stream_settings: Settings,
) -> ProtectionSettings {
	ProtectionSettings {
		_auth_settings,
		_actor_settings,
		_stream_settings,
	}
}

#[rstest::fixture]
fn malformed_json_router() -> reinhardt::ServerRouter {
	reinhardt::ServerRouter::new().handler("/broken", MalformedJson)
}

#[rstest::fixture]
async fn malformed_json_server(
	malformed_json_router: reinhardt::ServerRouter,
) -> (
	reinhardt::test::fixtures::server::TestServerGuard,
	reinhardt::test::APIClient,
) {
	let server = reinhardt::test::fixtures::server::test_server_guard(malformed_json_router).await;
	let client = reinhardt::test::fixtures::api_client_from_url(&server.url);
	(server, client)
}

#[rstest::rstest]
#[tokio::test]
async fn native_json_fixture_reports_malformed_responses(
	#[future(awt)] malformed_json_server: (
		reinhardt::test::fixtures::server::TestServerGuard,
		reinhardt::test::APIClient,
	),
) {
	let (_server, client) = malformed_json_server;
	assert!(client.get("/broken").await.unwrap().json_value().is_err());
}
