use super::*;
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
			Err(Error::External(_))
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
