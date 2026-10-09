use super::*;
use crate::context::{Context, ContextEvent, RequestBudget};
use crate::provider::{ContentPart, ModelRequest, ToolCall, ToolSpec};
use rstest::{fixture, rstest};

fn tools() -> Vec<ToolSpec> {
	vec![ToolSpec {
		name: "workspace_read".into(),
		description: "Read a record".into(),
		parameters: json!({"type":"object","properties":{"id":{"type":"string"}}}),
	}]
}

fn event(id: &str) -> ContextEvent {
	ContextEvent::tool(
		ToolCall {
			id: id.into(),
			name: "workspace_read".into(),
			arguments: json!({"id":id}),
		},
		json!({"content":format!("record {id} \"quoted\" 日本語")}),
	)
}

#[fixture]
fn context() -> Context {
	Context {
		summary: "earlier work".into(),
		run_message_summary: "user goal".into(),
		history: vec![event("a"), event("b")],
		..Context::default()
	}
}

fn pinned(step: i64) -> Value {
	json!({
		"identity":{"node_id":"node-a","agent_id":"agent","agent_version":"1"},
		"task":{"id":"task","title":"Compare offers","revision":3},
		"reference_documents":"private reference",
		"workspace":{"events":[{"sequence":step}]},
		"agent_state":{"phase":"THINKING","step":step},
		"semantic_memory":{"memory":"recalled"},
	})
}

fn request(projection: ProjectionVersion, context: &Context, step: i64) -> ModelRequest {
	let tools = tools();
	RequestBudget {
		window: usize::MAX,
		instructions: "Cache scope: salt\n\nsystem",
		tools: &tools,
		max_output_tokens: 64,
		projection,
	}
	.request(context, &pinned(step))
}

/// The bytes a provider caches, in its render order: tools, system, then each
/// user-message text part. The HTTP body's JSON key order is irrelevant.
fn cached_order(request: &ModelRequest) -> Vec<String> {
	let body = request.input_body();
	let mut rendered = vec![
		body["tools"].to_string(),
		body["messages"][0]["content"].to_string(),
	];
	for part in body["messages"][1]["content"].as_array().unwrap() {
		rendered.push(part.to_string());
	}
	rendered
}

#[rstest]
fn legacy_requests_keep_the_single_alphabetical_json_message(context: Context) {
	let request = request(ProjectionVersion::Legacy, &context, 7);
	let legacy = json!({
		"current":pinned(7),
		"summary":context.summary,
		"run_message_summary":context.run_message_summary,
		"history":context.history,
	});
	assert_eq!(
		request.input_body()["messages"][1]["content"],
		json!(legacy.to_string())
	);
	assert!(
		serde_json::to_value(&request)
			.unwrap()
			.get("projection")
			.is_none()
	);
}

#[rstest]
fn consecutive_ordered_steps_share_every_part_before_the_current_state(context: Context) {
	let first = request(ProjectionVersion::Ordered, &context, 7);
	let mut next_context = context.clone();
	next_context.history.push(event("c"));
	let second = request(ProjectionVersion::Ordered, &next_context, 8);

	let first = cached_order(&first);
	let second = cached_order(&second);
	let shared = first.len() - 1;
	assert_eq!(first[..shared], second[..shared]);
	// tools, system, Run context, history a, history b; then history c and the
	// new current state are the only additions.
	assert_eq!(shared, 5);
	assert!(second[shared].contains("\\\"id\\\":\\\"c\\\""));
	assert!(second[shared + 1].contains("\\\"step\\\":8"));
}

#[rstest]
fn ordered_parts_put_run_stable_context_first_and_step_state_last(context: Context) {
	let texts = ordered_texts(&request(ProjectionVersion::Ordered, &context, 7).context);
	assert_eq!(texts.len(), 4);
	let keys = |text: &str| {
		let order = [
			"identity",
			"task",
			"reference_documents",
			"run_message_summary",
			"summary",
		];
		order.map(|key| text.find(&format!("\"{key}\":")).unwrap())
	};
	assert!(keys(&texts[0]).is_sorted());
	assert!(texts[1].starts_with("{\"history\":{"));
	let current = &texts[3];
	assert!(current.starts_with("{\"current\":{\"agent_state\":"));
	assert!(
		current.find("\"workspace\":").unwrap() < current.find("\"semantic_memory\":").unwrap()
	);
	assert!(!texts[0].contains("agent_state") && !texts[0].contains("semantic_memory"));
}

#[rstest]
fn ordered_estimates_cover_the_complete_body_and_each_history_part(context: Context) {
	let request = request(ProjectionVersion::Ordered, &context, 7);
	assert!(request.estimated_total_tokens() >= request.input_body().to_string().len());

	let event = event("c");
	let mut after = context.clone();
	after.history.push(event.clone());
	let ordered = budget(ProjectionVersion::Ordered);
	let delta = request_size(&ordered.request(&after, &Value::Null))
		- request_size(&ordered.request(&context, &Value::Null));
	assert_eq!(
		crate::context::tool_event_growth(&context, &event, ProjectionVersion::Ordered),
		delta
	);
	assert!(delta > crate::context::tool_event_growth(&context, &event, ProjectionVersion::Legacy));
}

fn budget(projection: ProjectionVersion) -> RequestBudget<'static> {
	RequestBudget {
		window: usize::MAX,
		instructions: "",
		tools: &[],
		max_output_tokens: 0,
		projection,
	}
}

fn request_size(request: &ModelRequest) -> usize {
	request.input_body().to_string().len()
}

#[rstest]
fn ordered_media_follows_the_current_state(context: Context) {
	let mut request = request(ProjectionVersion::Ordered, &context, 7);
	request.content_parts = vec![ContentPart::Text("caption".into())];
	let content = request.input_body()["messages"][1]["content"].clone();
	let parts = content.as_array().unwrap();
	assert_eq!(parts.len(), 5);
	assert_eq!(parts[4], json!({"type":"text","text":"caption"}));
	assert!(request.estimated_total_tokens() >= request.input_body().to_string().len());
}

#[test]
fn the_cache_salt_line_has_one_width_for_every_key_version() {
	let line = cache_salt_line(u32::MAX, &[0xab; CACHE_SALT_MAC_BYTES]);
	assert_eq!(line.len(), cache_salt_placeholder().len());
	assert!(line.starts_with("Cache scope: kffffffff."));
}
