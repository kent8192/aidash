use super::*;
use aidash_application::provider_access::KeylessAccess;
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
		}
	}

	fn mcp(endpoint: String) -> ToolConfig {
		ToolConfig::Mcp {
			endpoint,
			tool_name: "echo".into(),
			replay: "read_only".into(),
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
				.invoke(&http(format!("{base}/echo")), input.clone(), "key-1")
				.await
				.unwrap(),
			input
		);
		let result = tools
			.invoke(&mcp(format!("{base}/mcp")), input.clone(), "key-2")
			.await
			.unwrap();
		assert_eq!(result["content"][0]["text"], input.to_string());
		server.abort();
	}

	#[tokio::test]
	async fn an_address_literal_that_is_not_permitted_is_refused_before_connecting() {
		let (listener, port) = serve().await;
		let tools = tools(&[]);
		for config in [
			http(format!("http://127.0.0.1:{port}/echo")),
			mcp(format!("http://127.0.0.1:{port}/mcp")),
		] {
			assert!(matches!(
				tools.invoke(&config, json!({}), "key").await,
				Err(Error::Forbidden)
			));
		}
		never_accepts(&listener).await;
	}

	#[tokio::test]
	async fn permitting_one_origin_does_not_permit_another_port() {
		let (listener, port) = serve().await;
		let tools = tools(&[&format!("http://127.0.0.1:{}", port.wrapping_add(1))]);
		assert!(matches!(
			tools
				.invoke(
					&http(format!("http://127.0.0.1:{port}/echo")),
					json!({}),
					"key"
				)
				.await,
			Err(Error::Forbidden)
		));
		never_accepts(&listener).await;
	}

	#[tokio::test]
	async fn a_name_resolving_to_a_private_address_is_refused_for_http_and_mcp() {
		let (listener, port) = serve().await;
		let tools = tools(&[]);
		for config in [
			http(format!("http://localhost:{port}/echo")),
			mcp(format!("http://localhost:{port}/mcp")),
		] {
			assert!(matches!(
				tools.invoke(&config, json!({}), "key").await,
				Err(Error::Forbidden)
			));
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
				tools.invoke(&config, json!({}), "key").await,
				Err(Error::External(_))
			));
		}
		server.abort();
	}

	#[tokio::test]
	async fn an_unreachable_permitted_origin_stays_an_outage() {
		let (listener, port) = serve().await;
		drop(listener);
		let base = format!("http://127.0.0.1:{port}");
		let tools = tools(&[&base]);
		for config in [http(format!("{base}/echo")), mcp(format!("{base}/mcp"))] {
			assert!(matches!(
				tools.invoke(&config, json!({}), "key").await,
				Err(Error::External(_))
			));
		}
	}
}
