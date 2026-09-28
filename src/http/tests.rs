use super::*;
use axum::routing::{get, post};
use axum_test::TestServer;
use futures_util::StreamExt;
use tower::ServiceExt;

fn request(path: &str) -> Request {
	Request::builder().uri(path).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn request_ids_and_sensitive_headers_cover_success_and_errors() {
	let router = Router::new().route(
		"/ok",
		get(|headers: axum::http::HeaderMap| async move {
			assert!(headers[header::AUTHORIZATION].is_sensitive());
			assert!(headers[header::COOKIE].is_sensitive());
			assert!(headers["x-aidash-csrf"].is_sensitive());
			([(header::SET_COOKIE, "session=secret; HttpOnly")], "ok")
		}),
	);
	let app = protect(router, &Settings::default());
	let id = "83cf2292-f297-4f85-bf57-52a546674488";
	let response = app
		.clone()
		.oneshot(
			Request::builder()
				.uri("/ok")
				.header("x-request-id", id)
				.header(header::AUTHORIZATION, "Bearer secret")
				.header(header::COOKIE, "session=secret")
				.header("x-aidash-csrf", "secret")
				.body(Body::empty())
				.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(response.status(), 200);
	assert_eq!(response.headers()["x-request-id"], id);
	assert_eq!(
		response.headers()[header::X_CONTENT_TYPE_OPTIONS],
		"nosniff"
	);
	assert!(response.headers()[header::SET_COOKIE].is_sensitive());
	let response = app
		.oneshot(
			Request::builder()
				.uri("/missing?secret=hidden")
				.header("x-request-id", "untrusted text")
				.body(Body::empty())
				.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(response.status(), 404);
	uuid::Uuid::parse_str(response.headers()["x-request-id"].to_str().unwrap()).unwrap();
}

#[tokio::test]
async fn body_limit_applies_to_declared_and_streamed_bodies() {
	let app = protect(
		Router::new()
			.route(
				"/body",
				post(|body: axum::body::Bytes| async move { body.len().to_string() }),
			)
			.layer(body_limit(BODY_LIMIT)),
		&Settings::default(),
	);
	let server = TestServer::new(app.clone()).unwrap();
	server
		.post("/body")
		.bytes(vec![b'x'; BODY_LIMIT].into())
		.await
		.assert_status_ok();
	server
		.post("/body")
		.bytes(vec![b'x'; BODY_LIMIT + 1].into())
		.await
		.assert_status_payload_too_large();
	let body = Body::from_stream(futures_util::stream::iter([
		Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![b'x'; BODY_LIMIT])),
		Ok(axum::body::Bytes::from_static(b"x")),
	]));
	let response = app
		.oneshot(
			Request::builder()
				.method("POST")
				.uri("/body")
				.body(body)
				.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(response.status(), 413);
	assert!(response.headers().contains_key("x-request-id"));
}

#[tokio::test]
async fn route_groups_preserve_declared_and_streamed_body_limits() {
	const CHUNK_BODY_LIMIT: usize = 6 * 1024 * 1024;
	async fn receive(body: axum::body::Bytes) -> String {
		body.len().to_string()
	}
	let app = protect(
		Router::new()
			.route("/body", post(receive))
			.fallback(receive)
			.layer(body_limit(BODY_LIMIT))
			.nest(
				"/api",
				Router::new()
					.route("/references/{id}/chunks", axum::routing::any(receive))
					.route_layer(body_limit(CHUNK_BODY_LIMIT)),
			)
			.nest(
				"/federation/v0.1",
				Router::new()
					.route("/scoped/files/chunk", axum::routing::any(receive))
					.route_layer(body_limit(CHUNK_BODY_LIMIT)),
			),
		&Settings::default(),
	);
	for (method, path, limit) in [
		(
			"POST",
			"/api/references/reference-id/chunks",
			CHUNK_BODY_LIMIT,
		),
		(
			"POST",
			"/federation/v0.1/scoped/files/chunk",
			CHUNK_BODY_LIMIT,
		),
		(
			"PUT",
			"/api/references/reference-id/chunks",
			CHUNK_BODY_LIMIT,
		),
		("POST", "/body", BODY_LIMIT),
		(
			"POST",
			"/api/references/reference-id/chunks/extra",
			BODY_LIMIT,
		),
		(
			"POST",
			"/federation/v0.1/scoped/files/chunk/extra",
			BODY_LIMIT,
		),
	] {
		for declared in [true, false] {
			for (size, status) in [(limit, 200), (limit + 1, 413)] {
				let mut request = Request::builder().method(method).uri(path);
				let body = if declared {
					request = request.header(header::CONTENT_LENGTH, size);
					Body::from(vec![b'x'; size])
				} else {
					Body::from_stream(futures_util::stream::iter([
						Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![b'x'; size - 1])),
						Ok(axum::body::Bytes::from_static(b"x")),
					]))
				};
				let response = app
					.clone()
					.oneshot(request.body(body).unwrap())
					.await
					.unwrap();
				assert_eq!(
					response.status(),
					status,
					"{method} {path}, declared={declared}, size={size}"
				);
				assert!(response.headers().contains_key("x-request-id"));
			}
		}
	}
}

#[tokio::test(start_paused = true)]
async fn timeout_releases_shared_admission_and_remains_observable() {
	let settings = Settings {
		concurrency: 1,
		timeout: Duration::from_secs(1),
		..Default::default()
	};
	let app = protect(
		Router::new()
			.route(
				"/slow",
				get(|| async {
					std::future::pending::<()>().await;
					"never"
				}),
			)
			.route("/fast", get(|| async { "ok" })),
		&settings,
	);
	let response = app.clone().oneshot(request("/slow")).await.unwrap();
	assert_eq!(response.status(), 504);
	assert!(response.headers().contains_key("x-request-id"));
	assert_eq!(app.oneshot(request("/fast")).await.unwrap().status(), 200);
}

#[tokio::test]
async fn concurrency_is_shared_across_routes_and_rejects_without_waiting() {
	let started = Arc::new(tokio::sync::Notify::new());
	let release = Arc::new(tokio::sync::Notify::new());
	let started_handler = started.clone();
	let release_handler = release.clone();
	let app = protect(
		Router::new()
			.route(
				"/first",
				get(move || {
					let started = started_handler.clone();
					let release = release_handler.clone();
					async move {
						started.notify_one();
						release.notified().await;
						"ok"
					}
				}),
			)
			.route("/second", get(|| async { "ok" })),
		&Settings {
			concurrency: 1,
			..Default::default()
		},
	);
	let first = tokio::spawn(app.clone().oneshot(request("/first")));
	started.notified().await;
	let response = tokio::time::timeout(
		Duration::from_secs(1),
		app.clone().oneshot(request("/second")),
	)
	.await
	.unwrap()
	.unwrap();
	assert_eq!(response.status(), 503);
	assert_eq!(response.headers()[header::RETRY_AFTER], "1");
	assert!(response.headers().contains_key("x-request-id"));
	release.notify_one();
	assert_eq!(first.await.unwrap().unwrap().status(), 200);
	assert_eq!(app.oneshot(request("/second")).await.unwrap().status(), 200);
}

#[tokio::test(start_paused = true)]
async fn sse_holds_its_slot_until_body_drop_without_holding_http_slot() {
	let slots = SseSlots(Arc::new(tokio::sync::Semaphore::new(1)));
	let sse = Router::new()
		.route(
			"/stream",
			get(|| async {
				axum::response::Sse::new(
					futures_util::stream::once(async {
						Ok::<_, std::convert::Infallible>(
							axum::response::sse::Event::default().id("7").data("hello"),
						)
					})
					.chain(futures_util::stream::pending()),
				)
			}),
		)
		.layer(middleware::from_fn_with_state(slots, sse_admission));
	let app = protect(
		sse.route("/fast", get(|| async { "ok" })),
		&Settings {
			concurrency: 1,
			timeout: Duration::from_secs(1),
			..Default::default()
		},
	);
	let response = app.clone().oneshot(request("/stream")).await.unwrap();
	assert_eq!(response.status(), 200);
	assert_eq!(
		app.clone()
			.oneshot(request("/stream"))
			.await
			.unwrap()
			.status(),
		503
	);
	// Dropping an unpolled response must release the permit too.
	drop(response);
	let response = app.clone().oneshot(request("/stream")).await.unwrap();
	assert_eq!(response.status(), 200);
	let mut stream = response.into_body().into_data_stream();
	let frame = stream.next().await.unwrap().unwrap();
	assert!(std::str::from_utf8(&frame).unwrap().contains("id: 7"));
	tokio::time::advance(Duration::from_secs(5)).await;
	assert_eq!(
		app.clone()
			.oneshot(request("/fast"))
			.await
			.unwrap()
			.status(),
		200
	);
	assert_eq!(
		app.clone()
			.oneshot(request("/stream"))
			.await
			.unwrap()
			.status(),
		503
	);
	drop(stream);
	assert_eq!(app.oneshot(request("/stream")).await.unwrap().status(), 200);
}

#[tokio::test]
async fn actor_limits_share_routes_but_separate_tenants() {
	let app = rate_limit(
		Router::new()
			.route("/a", get(|| async { "ok" }))
			.route("/b", get(|| async { "ok" })),
		ActorKey,
		1,
		Duration::from_secs(60),
	);
	let request_for = |path: &str, tenant: &str| {
		let mut request = request(path);
		request.extensions_mut().insert(Actor::Subject(
			crate::authorization::identity::SubjectIdentity {
				credential_id: uuid::Uuid::new_v4(),
				tenant: tenant.into(),
				subject: "alice".into(),
			},
		));
		request
	};
	assert_eq!(
		app.clone()
			.oneshot(request_for("/a", "first"))
			.await
			.unwrap()
			.status(),
		200
	);
	let denied = app
		.clone()
		.oneshot(request_for("/b", "first"))
		.await
		.unwrap();
	assert_eq!(denied.status(), 429);
	assert!(denied.headers().contains_key(header::RETRY_AFTER));
	assert_eq!(
		app.oneshot(request_for("/b", "second"))
			.await
			.unwrap()
			.status(),
		200
	);
}

#[tokio::test]
async fn login_limit_uses_peer_address_and_ignores_forwarded_headers() {
	let app = rate_limit(
		Router::new().route("/login", get(|| async { "ok" })),
		AuthKey(Vec::new()),
		1,
		Duration::from_secs(60),
	);
	let for_peer = |peer: &str, forwarded: &str| {
		let mut request = request("/login");
		request
			.headers_mut()
			.insert("x-forwarded-for", forwarded.parse().unwrap());
		request
			.headers_mut()
			.insert("x-real-ip", forwarded.parse().unwrap());
		request.extensions_mut().insert(axum::extract::ConnectInfo(
			peer.parse::<std::net::SocketAddr>().unwrap(),
		));
		request
	};
	assert_eq!(
		app.clone()
			.oneshot(for_peer("127.0.0.1:1", "1.2.3.4"))
			.await
			.unwrap()
			.status(),
		200
	);
	assert_eq!(
		app.clone()
			.oneshot(for_peer("127.0.0.1:2", "4.3.2.1"))
			.await
			.unwrap()
			.status(),
		429
	);
	assert_eq!(
		app.oneshot(for_peer("127.0.0.2:1", "4.3.2.1"))
			.await
			.unwrap()
			.status(),
		200
	);
}

#[tokio::test]
async fn metrics_use_route_templates_and_bounded_fallback() {
	let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
	let handle = recorder.handle();
	let _guard = metrics::set_default_local_recorder(&recorder);
	let server = TestServer::new(protect(
		Router::new().route("/items/{id}", get(|| async { "ok" })),
		&Settings::default(),
	))
	.unwrap();
	server
		.get("/items/secret-id?token=hidden")
		.await
		.assert_status_ok();
	server.get("/private-path").await.assert_status_not_found();
	let metrics = handle.render();
	assert!(metrics.contains("/items/{id}"), "{metrics}");
	assert!(metrics.contains("unmatched"), "{metrics}");
	assert!(!metrics.contains("secret-id"));
	assert!(!metrics.contains("private-path"));
	assert!(!metrics.contains("hidden"));
}

#[tokio::test]
async fn default_info_logs_correlate_responses_without_recording_secrets() {
	use std::io::Write;
	#[derive(Clone)]
	struct Writer(Arc<std::sync::Mutex<Vec<u8>>>);
	impl Write for Writer {
		fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
			self.0.lock().unwrap().extend_from_slice(bytes);
			Ok(bytes.len())
		}
		fn flush(&mut self) -> std::io::Result<()> {
			Ok(())
		}
	}
	let bytes = Arc::new(std::sync::Mutex::new(Vec::new()));
	let output = Writer(bytes.clone());
	let subscriber = tracing_subscriber::fmt()
		.with_ansi(false)
		.without_time()
		.with_env_filter("aidash=info")
		.with_writer(move || output.clone())
		.finish();
	tracing::subscriber::set_global_default(subscriber).unwrap();
	let app = protect(
		Router::new().route("/items/{id}", post(|| async { "ok" })),
		&Settings::default(),
	);
	let id = "83cf2292-f297-4f85-bf57-52a546674488";
	let response = app
		.oneshot(
			Request::builder()
				.method("POST")
				.uri("/items/private-id?token=private-query")
				.header("x-request-id", id)
				.header("authorization", "Bearer private-token")
				.header("cookie", "session=private-cookie")
				.body(Body::from("private-body"))
				.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(response.status(), 200);
	let log = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
	for expected in [
		id,
		"/items/{id}",
		"status=200",
		"duration_ms=",
		"HTTP response",
	] {
		assert!(log.contains(expected), "{log}");
	}
	for secret in [
		"private-id",
		"private-query",
		"private-token",
		"private-cookie",
		"private-body",
	] {
		assert!(!log.contains(secret), "{log}");
	}
}
