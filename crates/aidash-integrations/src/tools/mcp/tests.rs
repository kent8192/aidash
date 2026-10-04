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
		let client = BoundedClient(reqwest::Client::new());
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
