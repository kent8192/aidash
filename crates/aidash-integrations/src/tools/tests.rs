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
	let tools = HttpTools {
		client: reqwest::Client::new(),
	};
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
