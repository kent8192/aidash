//! Viewing a PDF page that carries no text through the real Harness
//! (ADR 0053).
use super::*;

const REPORT: &str = "https://research-fixture.example/annual-report.pdf";

/// The first `document_id` value in a request body, as the model would read
/// it inside a tool result (where its quotes are escaped).
fn document_id(body: &str) -> Option<String> {
	body.match_indices("document_id").find_map(|(start, key)| {
		let value = body[start + key.len()..].trim_start_matches(['\\', '"', ':']);
		let candidate = value.get(..36)?;
		Uuid::parse_str(candidate).ok()?;
		Some(candidate.to_owned())
	})
}

/// Read the report, view its second page, then answer.
fn viewing_router() -> Arc<Router> {
	let provider = reinhardt::test::stub::StubRouter::new()
		.route(
			"/v1/chat/completions",
			http::Method::POST,
			reply(move |request: reinhardt::Request| async move {
				let body = request.json::<Value>().unwrap().to_string();
				let call = |id: &str, name: &str, arguments: Value| {
					("tool_calls", json!({"role":"assistant","content":null,"tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}}]}))
				};
				let (finish, message) = if body.contains("research_view") && body.contains("view-1") {
					("stop", json!({"role":"assistant","content":"The second page is a chart I could not read."}))
				} else if body.contains("page_images") {
					// Only a fetch result says whether page images are available;
					// the view tool's description already names pages_without_text.
					let document = document_id(&body).expect("the fetch result names its document");
					call("view-1", "research_view", json!({"document_id":document,"page":2}))
				} else {
					call("fetch-1", "research_fetch", json!({"url":REPORT}))
				};
				reinhardt::Response::ok()
					.with_json(&json!({
						"choices":[{"index":0,"finish_reason":finish,"message":message}],
						"usage":{"prompt_tokens":1,"completion_tokens":1}
					}))
					.unwrap()
			}),
		)
		.into_server_router();
	Arc::new(provider)
}

#[rstest::rstest]
#[tokio::test]
async fn a_model_without_image_input_is_refused_a_page_view(
	#[future]
	#[with(Default::default(), viewing_router())]
	research_fixture: Research,
) {
	// Arrange: the fixture Agent's model declares text input only.
	let r = research_fixture.await;

	// Act
	let run = r.delegate().await;
	let run = r.until_completed(run.id).await;

	// Assert: the fetch listed the page; the view was refused before rendering.
	let results = run
		.context
		.events()
		.filter_map(|event| match event {
			aidash_domain::context::ContextEvent::Tool { call, result } => {
				Some((call.name.clone(), result.clone()))
			}
			_ => None,
		})
		.collect::<Vec<_>>();
	let fetched = &results
		.iter()
		.find(|(name, _)| name == "research_fetch")
		.unwrap()
		.1;
	assert_eq!(
		fetched["data"]["pages_without_text"]["pages"],
		json!([2]),
		"{fetched}"
	);
	assert_eq!(
		fetched["data"]["pages_without_text"]["page_images"],
		"available"
	);
	let viewed = &results
		.iter()
		.find(|(name, _)| name == "research_view")
		.unwrap()
		.1;
	assert_eq!(viewed["error"]["code"], "image_input_required", "{viewed}");
	assert!(
		r.column(
			"research_page_images",
			retention::to_jsonb(retention::col("fragment_id")),
			run.id,
		)
		.await
		.is_empty()
	);

	// A revocation redacts the stored view result like every research result.
	let revoke = format!("/api/runs/{}/research-revocation", run.id);
	let (status, record) = request(
		&r.c.app,
		&r.c.token,
		"POST",
		&revoke,
		json!({"reason":"removal request"}),
	)
	.await;
	assert_eq!(status, 200, "{record}");
	let views = r
		.column("invocations", retention::col("result"), run.id)
		.await
		.into_iter()
		.filter(|result| result["operation"] == "view")
		.collect::<Vec<_>>();
	assert_eq!(views.len(), 1, "{views:?}");
	assert_eq!(views[0]["error"]["code"], "evidence_revoked", "{views:?}");
	r.close().await;
}
