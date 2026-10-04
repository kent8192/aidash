use crate::endpoint::{EndpointFixture, assert_json, endpoint, workspace};
use rstest::rstest;
use serde_json::json;

#[rstest]
#[tokio::test]
async fn public_health_and_openapi_use_the_registered_routes(#[future] endpoint: EndpointFixture) {
	// Arrange
	let app = endpoint.await;
	// Act
	let health = app.anonymous.get("/health").await.unwrap();
	let schema = app.anonymous.get("/api/openapi.json").await.unwrap();
	// Assert
	assert_eq!(assert_json(health, 200)["status"], "ok");
	let schema = assert_json(schema, 200);
	assert!(schema["paths"]["/api/workspaces"].is_object());
}

#[rstest]
#[tokio::test]
async fn event_queries_honor_workspace_and_cursor_parameters(#[future] endpoint: EndpointFixture) {
	// Arrange
	let app = endpoint.await;
	let selected = workspace(&app.operator, "Selected").await;
	let id = selected["id"].as_str().unwrap();
	workspace(&app.operator, "Excluded").await;
	assert_json(
		app.operator
			.patch(
				&format!("/api/workspaces/{id}"),
				&json!({"revision":0,"state":{"step":1}}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	// Act
	let first = assert_json(
		app.operator
			.get(&format!("/api/events?workspace_id={id}&after=0"))
			.await
			.unwrap(),
		200,
	);
	let cursor = first[0]["sequence"].as_i64().unwrap();
	let next = app
		.operator
		.get(&format!("/api/events?workspace_id={id}&after={cursor}"))
		.await
		.unwrap();
	// Assert
	assert_eq!(first.as_array().unwrap().len(), 2);
	assert!(
		first
			.as_array()
			.unwrap()
			.iter()
			.all(|event| event["workspace_id"] == id)
	);
	let next = assert_json(next, 200);
	assert_eq!(next.as_array().unwrap().len(), 1);
	assert_eq!(next[0]["kind"], "workspace.updated");
	assert!(next[0]["sequence"].as_i64().unwrap() > cursor);
}

#[rstest]
#[tokio::test]
async fn event_stream_flushes_live_changes_and_resumes_from_last_event_id(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange
	let app = endpoint.await;
	let selected = workspace(&app.operator, "Stream selection").await;
	let id = selected["id"].as_str().unwrap();
	workspace(&app.operator, "Excluded from stream").await;
	let client = reqwest::Client::new();
	let url = format!("{}/api/events/stream?workspace_id={id}", app.server.url);
	// Act
	let mut response = client
		.get(&url)
		.bearer_auth(&app.runtime.config.api_token)
		.send()
		.await
		.unwrap();
	// Assert
	assert_eq!(response.status(), 200);
	assert_eq!(response.headers()["content-type"], "text/event-stream");
	assert!(!response.headers().contains_key("content-length"));
	let first = next_frame(&mut response).await;
	assert!(first.contains("workspace.created"), "{first}");
	assert!(first.contains(id), "{first}");
	assert!(!first.contains("Excluded from stream"));
	let cursor = first
		.lines()
		.find_map(|line| line.strip_prefix("id: "))
		.unwrap();
	assert_json(
		app.operator
			.patch(
				&format!("/api/workspaces/{id}"),
				&json!({
					"revision":0, "state":{"live":true}
				}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let next = next_frame(&mut response).await;
	assert!(next.contains("workspace.updated"), "{next}");
	drop(response);
	let mut resumed = client
		.get(&url)
		.bearer_auth(&app.runtime.config.api_token)
		.header("Last-Event-ID", cursor)
		.send()
		.await
		.unwrap();
	let resumed_frame = next_frame(&mut resumed).await;
	assert!(
		resumed_frame.contains("workspace.updated"),
		"{resumed_frame}"
	);
	assert!(!resumed_frame.contains("workspace.created"));
}

async fn next_frame(response: &mut reqwest::Response) -> String {
	tokio::time::timeout(std::time::Duration::from_secs(5), async {
		let mut bytes = Vec::new();
		loop {
			let chunk = response.chunk().await.unwrap().expect("SSE remains open");
			bytes.extend_from_slice(&chunk);
			if bytes.ends_with(b"\n\n") {
				return String::from_utf8(bytes).unwrap();
			}
			assert!(bytes.len() < 65536, "bounded SSE fixture frame");
		}
	})
	.await
	.expect("an SSE frame must arrive without completing the stream")
}
