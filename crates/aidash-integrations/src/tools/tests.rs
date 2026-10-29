use super::*;
use aidash_application::provider_access::KeylessAccess;
use std::sync::Arc;
#[rstest::rstest]
#[tokio::test]
async fn web_fetch_retries_transient_statuses_and_reports_terminal_source_rejections() {
	use axum::{Router, extract::Path, http::StatusCode, routing::get};

	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(
			listener,
			Router::new().route(
				"/{status}",
				get(|Path(status): Path<u16>| async move {
					(StatusCode::from_u16(status).unwrap(), "source body")
				}),
			),
		)
		.await
		.unwrap();
	});
	let client = reqwest::Client::new();
	for status in [403, 404] {
		let url = reqwest::Url::parse(&format!("{endpoint}/{status}")).unwrap();
		let response = client.get(url.clone()).send().await.unwrap();
		assert_eq!(
			web_fetch_response(response, &url).await.unwrap(),
			json!({"ok":false,"error":{"kind":"http_status","url":url.as_str(),"status":status}})
		);
	}
	for status in [408, 429, 500] {
		let url = reqwest::Url::parse(&format!("{endpoint}/{status}")).unwrap();
		let response = client.get(url.clone()).send().await.unwrap();
		assert!(
			matches!(
				web_fetch_response(response, &url).await,
				Err(Error::External(_))
			),
			"HTTP {status} must remain on the worker retry path"
		);
	}
	let url = reqwest::Url::parse(&format!("{endpoint}/200")).unwrap();
	let response = client.get(url.clone()).send().await.unwrap();
	assert_eq!(
		web_fetch_response(response, &url).await.unwrap(),
		json!({"text":"source body"})
	);
	server.abort();
	let _ = server.await;
}

#[tokio::test(start_paused = true)]
async fn native_http_dns_resolution_is_bounded_by_the_tool_deadline() {
	let deadline = tokio::time::Instant::now() + NATIVE_HTTP_TIMEOUT;
	let stalled =
		std::future::pending::<std::io::Result<std::vec::IntoIter<std::net::SocketAddr>>>();
	let error = resolve_within(deadline, stalled).await.unwrap_err();
	assert!(matches!(error, Error::External(message) if message.contains("timed out")));
	assert!(tokio::time::Instant::now() >= deadline);
}

mod destinations {
	use super::*;
	use aidash_domain::keyless::PermittedOrigins;
	use axum::{
		Json, Router,
		http::StatusCode,
		response::{IntoResponse, Redirect, Response},
		routing::post,
	};
	use std::time::Duration;

	#[tokio::test]
	async fn mcp_call_http_status_obeys_replay_safety_and_retry_after() {
		use axum::extract::Path;
		async fn respond(Path(status): Path<u16>, Json(body): Json<Value>) -> Response {
			if body["method"] == "tools/call" {
				if (206..=210).contains(&status) {
					let id = if status != 207 {
						body["id"].clone()
					} else {
						json!(999999)
					};
					let event = json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":"x".repeat(256_001)}]}});
					let event_type = if status == 208 { "event: ping\n" } else { "" };
					let tail = match status {
						209 => "",
						210 => "\nevent: ping\n\n",
						_ => "\n\n",
					};
					return (
						[("content-type", "text/event-stream")],
						format!("{event_type}data: {event}{tail}"),
					)
						.into_response();
				}
				if status == 205 {
					return ([("content-type", "text/event-stream")], format!("data: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{{\"detail\":\"{}\"}}}}\n\n", "x".repeat(256_001))).into_response();
				}
				if matches!(status, 201 | 203) {
					let id = if status == 201 {
						body["id"].clone()
					} else {
						json!(999999)
					};
					return Json(json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":"x".repeat(256_001)}]}})).into_response();
				}
				if status == 600 {
					return Json(
						json!({"jsonrpc":"2.0","id":body["id"],"error":{"code":-32603,"message":"call rejected"}}),
					)
					.into_response();
				}
				return (
					StatusCode::from_u16(status).unwrap(),
					[("retry-after", "60")],
					"fixture failure",
				)
					.into_response();
			}
			echo_mcp(Json(body)).await
		}
		let (listener, port) = serve().await;
		let server = tokio::spawn(async move {
			axum::serve(listener, Router::new().route("/{status}", post(respond)))
				.await
				.unwrap()
		});
		let base = format!("http://127.0.0.1:{port}");
		let transport = tools(&[&base]);
		for (status, replay, retry) in [
			(422, "unsafe", false),
			(429, "unsafe", true),
			(503, "idempotent", true),
			(503, "unsafe", false),
		] {
			let config = ToolConfig::Mcp {
				endpoint: format!("{base}/{status}"),
				tool_name: "echo".into(),
				replay: replay.into(),
				idempotency_argument: (replay == "idempotent").then(|| "request_id".into()),
				secret: None,
			};
			let result = transport.invoke(&config, json!({}), "key", None).await;
			if retry {
				assert!(
					matches!(result, Err(Error::ToolNotDelivered { retry_after: Some(delay), .. }) if delay == Duration::from_secs(32)),
					"{status} {replay}"
				);
			} else {
				let output = result.unwrap();
				assert_eq!(output["error"]["status"], status);
				assert_eq!(output["error"]["body"], "fixture failure");
				if status == 503 {
					assert!(
						output["error"]["message"]
							.as_str()
							.unwrap()
							.contains("may have started")
					);
				}
			}
		}
		let config = mcp(format!("{base}/200"));
		let unreadable = transport.invoke(&config, json!({}), "key", None).await;
		assert!(matches!(unreadable, Err(Error::External(_))));
		let oversized_result = transport
			.invoke(&mcp(format!("{base}/201")), json!({}), "key", None)
			.await
			.unwrap();
		assert_eq!(
			oversized_result,
			json!({"message":"The call succeeded but its result could not be read."})
		);
		let wrong_id = transport
			.invoke(&mcp(format!("{base}/203")), json!({}), "key", None)
			.await;
		assert!(matches!(wrong_id, Err(Error::External(_))));
		let intermediate = tokio::time::timeout(
			Duration::from_secs(3),
			transport.invoke(&mcp(format!("{base}/205")), json!({}), "key", None),
		)
		.await
		.unwrap();
		assert!(matches!(intermediate, Err(Error::External(_))));
		let final_event = tokio::time::timeout(
			Duration::from_secs(3),
			transport.invoke(&mcp(format!("{base}/206")), json!({}), "key", None),
		)
		.await
		.unwrap()
		.unwrap();
		assert_eq!(
			final_event,
			json!({"message":"The call succeeded but its result could not be read."})
		);
		let wrong_event = tokio::time::timeout(
			Duration::from_secs(3),
			transport.invoke(&mcp(format!("{base}/207")), json!({}), "key", None),
		)
		.await
		.unwrap();
		assert!(matches!(wrong_event, Err(Error::External(_))));
		let control_event = tokio::time::timeout(
			Duration::from_secs(3),
			transport.invoke(&mcp(format!("{base}/208")), json!({}), "key", None),
		)
		.await
		.unwrap();
		assert!(matches!(control_event, Err(Error::External(_))));
		for status in [209, 210] {
			let interrupted = tokio::time::timeout(
				Duration::from_secs(10),
				transport.invoke(&mcp(format!("{base}/{status}")), json!({}), "key", None),
			)
			.await
			.unwrap();
			assert!(
				matches!(interrupted, Err(Error::External(_))),
				"status {status}"
			);
		}
		let rejected = transport
			.invoke(&mcp(format!("{base}/600")), json!({}), "key", None)
			.await
			.unwrap();
		assert_eq!(
			rejected,
			json!({"error":{"kind":"mcp_error","message":"call rejected"}})
		);
		server.abort();
	}

	#[tokio::test]
	async fn retryable_http_responses_and_connection_refusal_are_not_delivered() {
		let (listener, port) = serve().await;
		let server = tokio::spawn(async move {
			axum::serve(
				listener,
				Router::new()
					.route(
						"/rate",
						post(|| async {
							(
								StatusCode::TOO_MANY_REQUESTS,
								[("retry-after", "99999999999999999999999999")],
								"busy",
							)
						}),
					)
					.route(
						"/unavailable",
						post(|| async { (StatusCode::SERVICE_UNAVAILABLE, "busy") }),
					)
					.route(
						"/timeout",
						post(|| async { (StatusCode::REQUEST_TIMEOUT, "timeout") }),
					),
			)
			.await
			.unwrap()
		});
		let base = format!("http://127.0.0.1:{port}");
		let transport = tools(&[&base]);
		let rate = ToolConfig::Http {
			endpoint: format!("{base}/rate"),
			replay: "unsafe".into(),
			secret: None,
		};
		let error = transport
			.invoke(&rate, json!({}), "key", None)
			.await
			.unwrap_err();
		assert!(
			matches!(error, Error::ToolNotDelivered { retry_after: Some(delay), .. } if delay == Duration::from_secs(32))
		);
		let timeout = ToolConfig::Http {
			endpoint: format!("{base}/timeout"),
			replay: "unsafe".into(),
			secret: None,
		};
		assert!(matches!(
			transport.invoke(&timeout, json!({}), "key", None).await,
			Err(Error::ToolNotDelivered { .. })
		));
		let idempotent = ToolConfig::Http {
			endpoint: format!("{base}/unavailable"),
			replay: "idempotent".into(),
			secret: None,
		};
		let error = transport
			.invoke(&idempotent, json!({}), "key", None)
			.await
			.unwrap_err();
		assert!(matches!(error, Error::ToolNotDelivered { .. }));
		server.abort();
		let _ = server.await;
		let error = transport
			.invoke(&http(base), json!({}), "key", None)
			.await
			.unwrap_err();
		assert!(matches!(error, Error::ToolNotDelivered { .. }));
	}

	#[tokio::test]
	async fn refused_http_destination_is_a_failed_call() {
		let (listener, port) = serve().await;
		let result = tools(&[])
			.invoke(
				&http(format!("http://127.0.0.1:{port}/")),
				json!({}),
				"key",
				None,
			)
			.await
			.unwrap();
		assert_eq!(result["error"]["kind"], "outbound_policy");
		never_accepts(&listener).await;
	}

	#[tokio::test]
	async fn unsafe_http_server_failure_warns_that_change_may_have_started() {
		let (listener, port) = serve().await;
		let server = tokio::spawn(async move {
			axum::serve(
				listener,
				Router::new().route(
					"/",
					post(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "failure") }),
				),
			)
			.await
			.unwrap()
		});
		let endpoint = format!("http://127.0.0.1:{port}");
		let config = ToolConfig::Http {
			endpoint: endpoint.clone(),
			replay: "unsafe".into(),
			secret: None,
		};
		let result = tools(&[&endpoint])
			.invoke(&config, json!({}), "key", None)
			.await
			.unwrap();
		assert_eq!(result["error"]["status"], 500);
		assert_eq!(result["error"]["kind"], "http_status");
		assert!(
			result["error"]["message"]
				.as_str()
				.unwrap()
				.contains("may have started")
		);
		server.abort();
	}

	#[tokio::test]
	async fn http_success_with_unreadable_result_is_completed() {
		let (listener, port) = serve().await;
		let server = tokio::spawn(async move {
			axum::serve(
				listener,
				Router::new()
					.route("/", post(|| async { "not json" }))
					.route(
						"/oversized",
						post(|| async { Json(json!({"value":"x".repeat(256_001)})) }),
					),
			)
			.await
			.unwrap()
		});
		let endpoint = format!("http://127.0.0.1:{port}");
		let result = tools(&[&endpoint])
			.invoke(&http(endpoint.clone()), json!({}), "key", None)
			.await
			.unwrap();
		assert_eq!(
			result,
			json!({"message":"The call succeeded but its result could not be read."})
		);
		let oversized = tools(&[&endpoint])
			.invoke(
				&http(format!("{endpoint}/oversized")),
				json!({}),
				"key",
				None,
			)
			.await
			.unwrap();
		assert_eq!(oversized, result);
		server.abort();
	}

	#[tokio::test]
	async fn http_validation_failure_is_returned_as_a_tool_result() {
		let (listener, port) = serve().await;
		let server = tokio::spawn(async move {
			axum::serve(
				listener,
				Router::new()
					.route(
						"/",
						post(|| async { (StatusCode::UNPROCESSABLE_ENTITY, "invalid input") }),
					)
					.route(
						"/long",
						post(|| async {
							(
								StatusCode::UNPROCESSABLE_ENTITY,
								format!("{}🦀", "x".repeat(4095)),
							)
						}),
					),
			)
			.await
			.unwrap()
		});
		let endpoint = format!("http://127.0.0.1:{port}");
		let result = tools(&[&endpoint])
			.invoke(&http(endpoint.clone()), json!({}), "key", None)
			.await
			.unwrap();
		assert_eq!(
			result,
			json!({"error":{"kind":"http_status","status":422,"body":"invalid input"}})
		);
		let truncated = tools(&[&endpoint])
			.invoke(&http(format!("{endpoint}/long")), json!({}), "key", None)
			.await
			.unwrap();
		assert_eq!(
			truncated["error"]["body"].as_str().unwrap(),
			"x".repeat(4095)
		);
		server.abort();
	}

	fn tools(permitted: &[&str]) -> HttpTools {
		HttpTools::new(
			KeylessAccess::permitting(PermittedOrigins::parse(permitted).unwrap()),
			ToolClients::new().unwrap(),
		)
	}

	fn http(endpoint: String) -> ToolConfig {
		ToolConfig::Http {
			endpoint,
			replay: "read_only".into(),
			secret: None,
		}
	}

	fn mcp(endpoint: String) -> ToolConfig {
		ToolConfig::Mcp {
			endpoint,
			tool_name: "echo".into(),
			replay: "read_only".into(),
			secret: None,
			idempotency_argument: None,
		}
	}

	async fn echo_mcp(Json(body): Json<Value>) -> Response {
		let Some(id) = body.get("id") else {
			return StatusCode::ACCEPTED.into_response();
		};
		let result = match body["method"].as_str() {
			Some("initialize") => json!({
				"protocolVersion": body["params"]["protocolVersion"],
				"capabilities": {"tools": {}},
				"serverInfo": {"name": "fixture", "version": "1.0.0"}
			}),
			Some("tools/call") => json!({
				"content": [{"type": "text", "text": body["params"]["arguments"].to_string()}],
				"isError": false
			}),
			method => panic!("unexpected MCP method {method:?}"),
		};
		Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
	}

	fn fixture() -> Router {
		Router::new()
			.route(
				"/echo",
				post(|Json(body): Json<Value>| async move { Json(body) }),
			)
			.route("/mcp", post(echo_mcp))
			.route("/moved", post(|| async { Redirect::temporary("/echo") }))
	}

	async fn serve() -> (tokio::net::TcpListener, u16) {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let port = listener.local_addr().unwrap().port();
		(listener, port)
	}

	async fn never_accepts(listener: &tokio::net::TcpListener) {
		assert!(
			tokio::time::timeout(Duration::from_millis(200), listener.accept())
				.await
				.is_err(),
			"no connection may reach the listener"
		);
	}

	#[tokio::test]
	async fn a_permitted_origin_is_reached_over_http_and_mcp() {
		let (listener, port) = serve().await;
		let server = tokio::spawn(async move { axum::serve(listener, fixture()).await.unwrap() });
		let tools = tools(&[&format!("http://127.0.0.1:{port}")]);
		let base = format!("http://127.0.0.1:{port}");
		let input = json!({"topic": "tools"});

		assert_eq!(
			tools
				.invoke(&http(format!("{base}/echo")), input.clone(), "key-1", None)
				.await
				.unwrap(),
			input
		);
		let result = tools
			.invoke(&mcp(format!("{base}/mcp")), input.clone(), "key-2", None)
			.await
			.unwrap();
		assert_eq!(result["content"][0]["text"], input.to_string());
		server.abort();
	}

	#[tokio::test]
	async fn legacy_public_http_endpoints_are_refused_for_http_and_mcp() {
		use aidash_application::ports::tools::{PluginServer, PluginSessions};
		use aidash_domain::agent_plugin::RemoteServer;

		let (listener, port) = serve().await;
		let address = listener.local_addr().unwrap();
		let server = tokio::spawn(async move { axum::serve(listener, fixture()).await.unwrap() });
		// Reach a local fixture if an old immutable snapshot bypasses admission.
		let permitted = reqwest::Client::new();
		let public = reqwest::Client::builder()
			.no_proxy()
			.resolve("public.example", address)
			.build()
			.unwrap();
		let tools = HttpTools::new(
			KeylessAccess::default(),
			ToolClients {
				mcp_permitted: permitted.clone(),
				mcp_public: public.clone(),
				permitted,
				public,
			},
		);
		let mut refused = Vec::new();
		for config in [
			http(format!("http://public.example:{port}/echo")),
			mcp(format!("http://public.example:{port}/mcp")),
		] {
			refused.push(
				tools.invoke(&config, json!({}), "key", None).await.unwrap()["error"]["kind"]
					== "outbound_policy",
			);
		}
		let sessions = crate::plugin_sessions::McpPluginSessions::new(
			KeylessAccess::default(),
			tools.clients.clone(),
		);
		let listed = sessions
			.list(
				uuid::Uuid::new_v4(),
				&PluginServer {
					plugin: "legacy".to_owned().try_into().unwrap(),
					digest: "sha256:fixture".into(),
					server: "public".to_owned().try_into().unwrap(),
				},
				&RemoteServer {
					url: format!("http://public.example:{port}/mcp").parse().unwrap(),
					headers: vec![],
				},
			)
			.await;
		server.abort();
		assert_eq!(refused, [true, true]);
		assert!(matches!(listed, Err(Error::Forbidden)), "{listed:?}");
	}

	#[tokio::test]
	async fn an_address_literal_that_is_not_permitted_is_refused_before_connecting() {
		let (listener, port) = serve().await;
		let tools = tools(&[]);
		for config in [
			http(format!("http://127.0.0.1:{port}/echo")),
			mcp(format!("http://127.0.0.1:{port}/mcp")),
		] {
			assert_eq!(
				tools.invoke(&config, json!({}), "key", None).await.unwrap()["error"]["kind"],
				"outbound_policy"
			);
		}
		never_accepts(&listener).await;
	}

	#[tokio::test]
	async fn permitting_one_origin_does_not_permit_another_port() {
		let (listener, port) = serve().await;
		let tools = tools(&[&format!("http://127.0.0.1:{}", port.wrapping_add(1))]);
		assert_eq!(
			tools
				.invoke(
					&http(format!("http://127.0.0.1:{port}/echo")),
					json!({}),
					"key",
					None,
				)
				.await
				.unwrap()["error"]["kind"],
			"outbound_policy"
		);
		never_accepts(&listener).await;
	}

	#[tokio::test]
	async fn a_name_resolving_to_a_private_address_is_refused_for_http_and_mcp() {
		let (listener, port) = serve().await;
		let tools = tools(&[]);
		for config in [
			http(format!("https://localhost:{port}/echo")),
			mcp(format!("https://localhost:{port}/mcp")),
		] {
			assert_eq!(
				tools.invoke(&config, json!({}), "key", None).await.unwrap()["error"]["kind"],
				"outbound_policy"
			);
		}
		never_accepts(&listener).await;
	}

	#[tokio::test]
	async fn a_permitted_origin_cannot_redirect_a_call_elsewhere() {
		let (listener, port) = serve().await;
		let server = tokio::spawn(async move { axum::serve(listener, fixture()).await.unwrap() });
		let base = format!("http://127.0.0.1:{port}");
		let tools = tools(&[&base]);
		for config in [http(format!("{base}/moved")), mcp(format!("{base}/moved"))] {
			assert!(matches!(
				tools.invoke(&config, json!({}), "key", None).await,
				Err(Error::External(_))
			));
		}
		server.abort();
	}

	#[tokio::test]
	async fn an_unreachable_permitted_origin_is_not_delivered() {
		let (listener, port) = serve().await;
		drop(listener);
		let base = format!("http://127.0.0.1:{port}");
		let tools = tools(&[&base]);
		for config in [http(format!("{base}/echo")), mcp(format!("{base}/mcp"))] {
			assert!(matches!(
				tools.invoke(&config, json!({}), "key", None).await,
				Err(Error::ToolNotDelivered { .. })
			));
		}
	}
}

/// Serves one JSON route that echoes the request's Authorization header, and
/// reports every header value it saw.
async fn authorization_echo() -> (String, tokio::task::JoinHandle<()>) {
	use axum::{Router, http::HeaderMap, routing::post};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/run", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(
			listener,
			Router::new().route(
				"/run",
				post(|headers: HeaderMap| async move {
					let seen = headers
						.get("authorization")
						.and_then(|v| v.to_str().ok())
						.map(str::to_owned);
					axum::Json(json!({"authorization": seen}))
				}),
			),
		)
		.await
		.unwrap();
	});
	(endpoint, server)
}

#[tokio::test]
async fn an_http_tool_sends_its_tenant_secret_as_the_bearer_credential_and_nothing_otherwise() {
	let (endpoint, server) = authorization_echo().await;
	let tools = HttpTools::new(permitting([endpoint.as_str()]), ToolClients::new().unwrap());
	let config = ToolConfig::Http {
		endpoint,
		replay: "idempotent".into(),
		secret: Some("search".into()),
	};
	let secret = SecretString::from("tenant-secret-value".to_owned());
	let with = tools
		.invoke(&config, json!({}), "key", Some(&secret))
		.await
		.unwrap();
	assert_eq!(with["authorization"], "Bearer tenant-secret-value");
	let without = tools.invoke(&config, json!({}), "key", None).await.unwrap();
	assert_eq!(without["authorization"], Value::Null);
	server.abort();
	let _ = server.await;
}

#[tokio::test]
async fn an_mcp_tool_authenticates_its_handshake_and_call_with_the_tenant_secret() {
	use axum::{
		Json, Router,
		http::{HeaderMap, StatusCode},
		response::IntoResponse,
		routing::post,
	};
	let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
	let observed = calls.clone();
	let (endpoint, server) = serve(Router::new().route("/mcp", post(move |headers: HeaderMap, Json(message): Json<Value>| {
		let calls = observed.clone();
		async move {
			assert_eq!(headers["authorization"], "Bearer tenant-mcp-canary-296");
			let result = match message["method"].as_str().unwrap() {
				"initialize" => json!({"protocolVersion":message["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}),
				"tools/call" => {
					assert_eq!(message["params"]["name"], "lookup");
					assert_eq!(message["params"]["arguments"]["request_id"], "call-key");
					assert!(!message.to_string().contains("tenant-mcp-canary-296"));
					calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
					json!({"content":[{"type":"text","text":"authenticated result"}]})
				},
				_ => return StatusCode::ACCEPTED.into_response(),
			};
			Json(json!({"jsonrpc":"2.0","id":message["id"],"result":result})).into_response()
		}
	}))).await;
	let tools = HttpTools::new(permitting([endpoint.as_str()]), ToolClients::new().unwrap());
	let config = ToolConfig::Mcp {
		endpoint,
		tool_name: "lookup".into(),
		secret: Some("mcp-key".into()),
		replay: "idempotent".into(),
		idempotency_argument: Some("request_id".into()),
	};
	let result = tools
		.invoke(
			&config,
			json!({"query":"fixture"}),
			"call-key",
			Some(&"tenant-mcp-canary-296".into()),
		)
		.await
		.unwrap();
	assert_eq!(result["content"][0]["text"], "authenticated result");
	assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
	server.abort();
	let _ = server.await;
}

/// An MCP server over streamable HTTP. `tools/list` answers `pages` as the
/// cursors `"1"`, `"2"` and so on chain them; the last page has no `nextCursor`
/// unless `endless` is set.
fn mcp_server(pages: Vec<Value>, endless: bool) -> axum::Router {
	use axum::{Json, http::StatusCode, response::IntoResponse, routing::post};

	axum::Router::new().route(
		"/mcp",
		post(move |Json(message): Json<Value>| {
			let pages = pages.clone();
			async move {
				let result = match message["method"].as_str().unwrap() {
					"initialize" => json!({
						"protocolVersion": message["params"]["protocolVersion"],
						"capabilities": {"tools": {}},
						"serverInfo": {"name": "fixture", "version": "1.0.0"}
					}),
					"tools/list" => {
						let page: usize = message["params"]["cursor"]
							.as_str()
							.map_or(0, |cursor| cursor.parse().unwrap());
						let next =
							(endless || page + 1 < pages.len()).then(|| (page + 1).to_string());
						json!({"tools": pages[page.min(pages.len() - 1)], "nextCursor": next})
					}
					_ => return StatusCode::ACCEPTED.into_response(),
				};
				Json(json!({"jsonrpc":"2.0","id":message["id"],"result":result})).into_response()
			}
		}),
	)
}

/// Keyless access that permits the origin of each endpoint, as the Deployment's
/// permitted private origins do (ADR 0036).
fn permitting<'a>(
	endpoints: impl IntoIterator<Item = &'a str>,
) -> aidash_application::provider_access::KeylessAccess {
	let origins = endpoints.into_iter().map(|endpoint| {
		reqwest::Url::parse(endpoint)
			.unwrap()
			.origin()
			.ascii_serialization()
	});
	aidash_application::provider_access::KeylessAccess::permitting(
		aidash_domain::keyless::PermittedOrigins::parse(origins).unwrap(),
	)
}

fn clients() -> Clients {
	Clients::new(
		reqwest::Client::new(),
		std::time::Duration::from_secs(5),
		std::time::Duration::from_secs(1),
	)
	.unwrap()
}

async fn serve(router: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener, router).await.unwrap();
	});
	(endpoint, server)
}

#[tokio::test]
async fn an_mcp_servers_read_only_hint_is_read_from_its_tool_list() {
	let (endpoint, server) = serve(mcp_server(
		vec![
			json!([
				{"name":"lookup","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}},
				{"name":"write","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":false}}
			]),
			json!([
				{"name":"titled","inputSchema":{"type":"object"},"annotations":{"title":"Titled"}},
				{"name":"plain","inputSchema":{"type":"object"}}
			]),
		],
		false,
	))
	.await;
	let (clients, origins) = (clients(), permitting([endpoint.as_str()]));
	for (tool, hint) in [
		("lookup", Some(true)),
		("write", Some(false)),
		("titled", None),
		("plain", None),
	] {
		assert_eq!(
			mcp_read_only_hint(&clients, &origins, &endpoint, tool)
				.await
				.unwrap(),
			hint,
			"{tool}: a later page is read when the tool is not on the first"
		);
	}
	assert!(matches!(
		mcp_read_only_hint(&clients, &origins, &endpoint, "missing").await,
		Err(Error::Invalid(_))
	));
	assert!(matches!(
		mcp_read_only_hint(&clients, &origins, "ftp://example.com/mcp", "lookup").await,
		Err(Error::Invalid(_))
	));
	let unreachable = "http://127.0.0.1:1/mcp";
	assert!(matches!(
		mcp_read_only_hint(&clients, &permitting([unreachable]), unreachable, "lookup").await,
		Err(Error::External(_))
	));
	server.abort();
	let _ = server.await;
}

#[tokio::test]
async fn a_server_that_never_ends_its_tool_list_is_read_a_bounded_number_of_pages() {
	let (endpoint, server) = serve(mcp_server(
		vec![json!([{"name":"other","inputSchema":{"type":"object"}}])],
		true,
	))
	.await;
	assert!(matches!(
		mcp_read_only_hint(
			&clients(),
			&permitting([endpoint.as_str()]),
			&endpoint,
			"lookup"
		)
		.await,
		Err(Error::Invalid(_))
	));
	server.abort();
	let _ = server.await;
}

/// The endpoint is the Tenant's choice and nothing approves it first, so an
/// address outside the Deployment's permitted origins never receives a request.
#[tokio::test]
async fn an_mcp_endpoint_outside_the_permitted_origins_is_never_contacted() {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let port = listener.local_addr().unwrap().port();
	let neighbour = format!("http://127.0.0.1:{}", port + 1);
	let clients = clients();
	for (endpoint, origins) in [
		(format!("http://127.0.0.1:{port}/mcp"), permitting([])),
		(format!("http://10.0.0.1:{port}/mcp"), permitting([])),
		(
			format!("http://127.0.0.1:{port}/mcp"),
			permitting([neighbour.as_str()]),
		),
	] {
		assert!(
			matches!(
				mcp_read_only_hint(&clients, &origins, &endpoint, "lookup").await,
				Err(Error::Forbidden)
			),
			"{endpoint}: a non-public address literal is forbidden"
		);
	}
	// A name is checked against every address it resolves to when connecting.
	let name = format!("http://localhost:{port}/mcp");
	assert!(
		matches!(
			mcp_read_only_hint(&clients, &permitting([]), &name, "lookup").await,
			Err(Error::Forbidden)
		),
		"a name resolving to loopback is refused"
	);
	assert!(
		tokio::time::timeout(std::time::Duration::from_millis(200), listener.accept())
			.await
			.is_err(),
		"no connection may reach the unpermitted listener"
	);
}

/// A permitted origin cannot hand the session on to another address.
#[tokio::test]
async fn a_permitted_mcp_origin_follows_no_redirects() {
	use axum::{Router, response::Redirect, routing::post};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(
			listener,
			Router::new().route("/mcp", post(|| async { Redirect::temporary("/elsewhere") })),
		)
		.await
		.unwrap();
	});
	assert!(matches!(
		mcp_read_only_hint(
			&clients(),
			&permitting([endpoint.as_str()]),
			&endpoint,
			"lookup"
		)
		.await,
		Err(Error::External(_))
	));
	server.abort();
	let _ = server.await;
}

#[tokio::test]
async fn an_unpermitted_plaintext_mcp_hint_is_refused_before_discovery() {
	let (endpoint, server) = serve(mcp_server(
		vec![
			json!([{"name":"lookup", "inputSchema":{"type":"object"}, "annotations":{"readOnlyHint":true}}]),
		],
		false,
	))
	.await;
	let address: std::net::SocketAddr = reqwest::Url::parse(&endpoint)
		.unwrap()
		.socket_addrs(|| None)
		.unwrap()[0];
	let endpoint = format!("http://public.example:{}/mcp", address.port());
	let public = reqwest::Client::builder()
		.no_proxy()
		.resolve("public.example", address)
		.build()
		.unwrap();
	let mut clients = clients();
	clients.public = public;
	let result = mcp_read_only_hint(&clients, &KeylessAccess::default(), &endpoint, "lookup").await;
	server.abort();
	assert!(matches!(result, Err(Error::Forbidden)), "{result:?}");
}
