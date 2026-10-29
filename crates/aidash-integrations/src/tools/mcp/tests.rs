use super::*;
use axum::{Router, routing::post};
use serde_json::json;

#[rstest::rstest]
#[tokio::test]
async fn oversized_json_and_unterminated_sse_are_rejected_before_decoding() {
	for content_type in ["application/json", "text/event-stream"] {
		let app = Router::new().route(
			"/mcp",
			post(move || async move {
				(
					[("content-type", content_type)],
					format!("data: {}", "x".repeat(LIMIT + 1)),
				)
			}),
		);
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let uri: Arc<str> = format!("http://{}/mcp", listener.local_addr().unwrap()).into();
		let server = tokio::spawn(async move {
			axum::serve(listener, app).await.unwrap();
		});
		let client = BoundedClient::direct(reqwest::Client::new());
		let message = serde_json::from_value(
			json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}),
		)
		.unwrap();
		let result = client
			.post_message(uri, message, None, None, HashMap::new())
			.await;
		if content_type == "application/json" {
			assert!(result.is_err());
		} else {
			let StreamableHttpPostResponse::Sse(mut stream, _) = result.unwrap() else {
				panic!("expected SSE")
			};
			let item = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
				.await
				.unwrap();
			assert!(item.unwrap().is_err());
		}
		server.abort();
	}
}

async fn refused_error() -> reqwest::Error {
	crate::tools::ToolClients::new()
		.unwrap()
		.public
		.get("http://localhost:9/")
		.send()
		.await
		.unwrap_err()
}

fn wrapped(error: reqwest::Error) -> DynamicTransportError {
	DynamicTransportError {
		transport_name: "test".into(),
		transport_type_id: std::any::TypeId::of::<Failure>(),
		error: Box::new(Failure::Client(McpError::Http(error))),
	}
}

#[tokio::test]
async fn a_refused_destination_is_an_authority_failure_in_every_rmcp_wrapper() {
	let initialization = ClientInitializeError::TransportError {
		error: wrapped(refused_error().await),
		context: "send initialize request".into(),
	};
	assert!(matches!(
		initialization_error(&initialization),
		Error::Forbidden
	));
	let call = ServiceError::TransportSend(wrapped(refused_error().await));
	assert!(matches!(call_error(&call), Error::Forbidden));
}

#[tokio::test]
async fn other_transport_failures_stay_outages() {
	let unreachable = || async {
		reqwest::Client::new()
			.get("http://127.0.0.1:9/")
			.timeout(std::time::Duration::from_secs(2))
			.send()
			.await
			.unwrap_err()
	};
	let initialization = ClientInitializeError::TransportError {
		error: wrapped(unreachable().await),
		context: "send initialize request".into(),
	};
	assert!(matches!(
		initialization_error(&initialization),
		Error::External(_)
	));
	assert!(matches!(
		call_error(&ServiceError::TransportSend(wrapped(unreachable().await))),
		Error::External(_)
	));
	assert!(matches!(
		call_error(&ServiceError::TransportClosed),
		Error::External(_)
	));
}

struct UnusedRouteSource;
#[async_trait::async_trait]
impl SecretRouteSource for UnusedRouteSource {
	async fn route(&self) -> aidash_application::Result<SecretRoute> {
		Err(Error::External("unexpected refresh".into()))
	}
}

#[tokio::test]
async fn brokered_mcp_refusal_preserves_the_reason_and_correlation() {
	let app = Router::new().route(
		"/forward/tool",
		post(|headers: axum::http::HeaderMap| async move {
			assert_eq!(
				headers
					.get("x-aidash-secret-call")
					.map(|v| v.to_str().unwrap()),
				Some("capability")
			);
			assert!(headers.get("authorization").is_none());
			(
				axum::http::StatusCode::FORBIDDEN,
				[
					("x-aidash-broker-error", "secret_destination_refused"),
					("x-aidash-correlation", "trace-1"),
				],
				"private broker body",
			)
		}),
	);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let base = format!("http://{}/forward", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
	let client = BoundedClient::brokered(
		reqwest::Client::new(),
		Arc::new(UnusedRouteSource),
		"tool".into(),
		SecretRoute {
			base: base.parse().unwrap(),
			token: "capability".into(),
		},
	);
	let message =
		serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}))
			.unwrap();
	let result = client
		.post_message(
			"http://unused.invalid".into(),
			message,
			None,
			Some("should-not-forward".into()),
			HashMap::new(),
		)
		.await;
	match result {
		Err(Failure::Client(McpError::Broker(failure))) => {
			assert_eq!(failure.code_str(), "secret_destination_refused");
			assert_eq!(failure.correlation.as_deref(), Some("trace-1"));
			assert!(
				matches!(failure.into_error(), Error::Invalid(code) if code == "secret_destination_refused")
			);
		}
		other => panic!("unexpected response: {other:?}"),
	}
	server.abort();
}
