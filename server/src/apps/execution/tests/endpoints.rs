use crate::endpoint::{EndpointFixture, assert_json, endpoint, thread};
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
	assert!(schema["paths"]["/api/threads"].is_object());
}

#[rstest]
#[tokio::test]
async fn event_queries_honor_thread_and_cursor_parameters(#[future] endpoint: EndpointFixture) {
	// Arrange
	let app = endpoint.await;
	let (member, _) = crate::endpoint::tenant_member(&app).await;
	let selected = thread(&member, "Selected").await;
	let id = selected["id"].as_str().unwrap();
	thread(&member, "Excluded").await;
	assert_json(
		member
			.patch(
				&format!("/api/threads/{id}"),
				&json!({"revision":0,"title":"Step"}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	// Act
	let first = assert_json(
		member
			.get(&format!("/api/events?thread_id={id}&after=0"))
			.await
			.unwrap(),
		200,
	)["events"]
		.clone();
	let cursor = first[0]["sequence"].as_i64().unwrap();
	let next = member
		.get(&format!("/api/events?thread_id={id}&after={cursor}"))
		.await
		.unwrap();
	// Assert
	assert_eq!(first.as_array().unwrap().len(), 2);
	assert!(
		first
			.as_array()
			.unwrap()
			.iter()
			.all(|event| event["thread_id"] == id)
	);
	let next = assert_json(next, 200)["events"].clone();
	assert_eq!(next.as_array().unwrap().len(), 1);
	assert_eq!(next[0]["kind"], "thread.updated");
	assert!(next[0]["sequence"].as_i64().unwrap() > cursor);
}

#[rstest]
#[tokio::test]
async fn event_stream_flushes_live_changes_and_resumes_from_last_event_id(
	#[future] endpoint: EndpointFixture,
	#[from(reinhardt::test::fixtures::http_client)] http_client: reqwest::Client,
) {
	// Arrange
	let app = endpoint.await;
	// The endpoint fixture has no broker listener. Reconcile sooner than the
	// frame deadline so this exercises fallback delivery deterministically.
	app.context
		.set_singleton(reinhardt::di::KeyedFactoryOutput::<
			reinhardt::di::SelfKey<aidash_server::sse::Service>,
			aidash_server::sse::Service,
		>::new(aidash_server::sse::Service::new(
			aidash_server::sse::Settings {
				reconcile_interval: std::time::Duration::from_millis(250),
				..Default::default()
			},
		)));
	let (member, token) = crate::endpoint::tenant_member(&app).await;
	let selected = thread(&member, "Stream selection").await;
	let id = selected["id"].as_str().unwrap();
	thread(&member, "Excluded from stream").await;
	let client = http_client.clone();
	let url = format!("{}/api/events/stream?thread_id={id}", app.server.url);
	// Act
	let mut response = client.get(&url).bearer_auth(&token).send().await.unwrap();
	// Assert
	assert_eq!(response.status(), 200);
	assert_eq!(response.headers()["content-type"], "text/event-stream");
	assert!(!response.headers().contains_key("content-length"));
	let first = next_frame(&mut response).await;
	assert!(first.contains("thread.created"), "{first}");
	assert!(first.contains(id), "{first}");
	assert!(!first.contains("Excluded from stream"));
	let cursor = first
		.lines()
		.find_map(|line| line.strip_prefix("id: "))
		.unwrap();
	assert_json(
		member
			.patch(
				&format!("/api/threads/{id}"),
				&json!({
					"revision":0, "title":"Live"
				}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let next = next_frame(&mut response).await;
	assert!(next.contains("thread.updated"), "{next}");
	drop(response);
	let mut resumed = client
		.get(&url)
		.bearer_auth(&token)
		.header("Last-Event-ID", cursor)
		.send()
		.await
		.unwrap();
	let resumed_frame = next_frame(&mut resumed).await;
	assert!(resumed_frame.contains("thread.updated"), "{resumed_frame}");
	assert!(!resumed_frame.contains("thread.created"));
}

#[rstest]
#[tokio::test]
async fn deployment_administrators_and_other_tenants_read_no_events_or_event_streams(
	#[future] endpoint: EndpointFixture,
	#[from(reinhardt::test::fixtures::http_client)] http_client: reqwest::Client,
) {
	// Arrange: a Tenant Subject has a Thread with events.
	let app = endpoint.await;
	let (member, _) = crate::endpoint::tenant_member(&app).await;
	let selected = thread(&member, "Private to the Tenant").await;
	let id = selected["id"].as_str().unwrap();
	let other = crate::endpoint::tenant_token(&app.runtime.store, "globex").await;
	let administrator = &app.runtime.config.api_token;
	let stream = format!("{}/api/events/stream?thread_id={id}", app.server.url);

	// Act and Assert: the shared bearer is refused on event reads.
	for path in [
		"/api/events".to_owned(),
		format!("/api/events?thread_id={id}&after=0"),
	] {
		let response = app.deployment_administrator.get(&path).await.unwrap();
		assert_eq!(response.status_code(), 403, "{path}");
		assert!(!response.text().contains(id), "{path}");
	}
	// It is refused on the event stream, fresh and when resuming from a cursor.
	for cursor in [None, Some("0"), Some("1")] {
		let mut request = http_client.get(&stream).bearer_auth(administrator);
		if let Some(cursor) = cursor {
			request = request.header("Last-Event-ID", cursor);
		}
		let response = request.send().await.unwrap();
		assert_eq!(response.status(), 403, "{cursor:?}");
		assert!(!response.text().await.unwrap().contains(id), "{cursor:?}");
	}
	// Without a Thread filter the stream is still Tenant content: refused.
	let unfiltered = http_client
		.get(format!("{}/api/events/stream", app.server.url))
		.bearer_auth(administrator)
		.send()
		.await
		.unwrap();
	assert_eq!(unfiltered.status(), 403, "unfiltered event stream");
	assert!(!unfiltered.text().await.unwrap().contains(id));
	// Another Tenant's Subject sees none of this Tenant's events.
	let response = http_client
		.get(format!(
			"{}/api/events?thread_id={id}&after=0",
			app.server.url
		))
		.bearer_auth(&other)
		.send()
		.await
		.unwrap();
	let status = response.status();
	let body = response.text().await.unwrap();
	assert!(
		!status.is_success() || body == r#"{"events":[],"unconfirmed":[]}"#,
		"{status}: {body}"
	);
	assert!(!body.contains(id), "{body}");
	// The authorized Tenant Subject still reads them.
	let events = assert_json(
		member
			.get(&format!("/api/events?thread_id={id}&after=0"))
			.await
			.unwrap(),
		200,
	);
	assert_eq!(events["events"].as_array().unwrap().len(), 1);
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
