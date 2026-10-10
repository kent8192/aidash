//! Explicit cache breakpoints (ADR 0019) end `system` and the Ordered Stable
//! Prefix part, never the volatile part, and never change an estimate.
use super::*;
use crate::context::{Context, ContextEvent, RequestBudget, RequestProjection};
use crate::projection::CacheScope;
use rstest::{fixture, rstest};

fn tools() -> Vec<ToolSpec> {
	vec![ToolSpec {
		name: "workspace_read".into(),
		description: "Read".into(),
		parameters: json!({"type":"object"}),
	}]
}

fn request(projection: RequestProjection, events: u32) -> ModelRequest {
	let context = Context {
		history: (1..=events)
			.map(|n| {
				ContextEvent::tool(
					ToolCall {
						id: format!("call-{n}"),
						name: "workspace_read".into(),
						arguments: json!({"kind":"task"}),
					},
					json!({"read":n}),
				)
			})
			.collect(),
		..Context::default()
	};
	let pinned = json!({"identity":{"agent_id":"agent"},"task":{"title":"Task"},"agent_state":{"step":events}});
	RequestBudget {
		window: usize::MAX,
		instructions: "Follow the task",
		tools: &tools(),
		max_output_tokens: 64,
		projection: &projection,
	}
	.request(&context, &pinned)
}

#[fixture]
fn ordered() -> ModelRequest {
	request(
		RequestProjection::Ordered(CacheScope {
			tenant: "tenant-a".into(),
			key_version: 1,
		}),
		2,
	)
}

/// Which body locations carry `cache_control`: `system`, then each user part.
fn marked(request: &ModelRequest) -> (bool, Vec<bool>) {
	let body = request.input_body();
	assert!(!body["tools"].to_string().contains("cache_control"));
	let system = body["messages"][0]["content"][0]
		.get("cache_control")
		.is_some();
	let parts = body["messages"][1]["content"]
		.as_array()
		.unwrap()
		.iter()
		.map(|part| part.get("cache_control").is_some())
		.collect();
	(system, parts)
}

#[rstest]
fn breakpoints_end_system_and_the_stable_part_but_never_the_volatile_part(
	mut ordered: ModelRequest,
) {
	assert_eq!(marked(&ordered), (false, vec![false, false]));
	assert!(!ordered.input_body().to_string().contains("cache_control"));
	ordered.cache_breakpoints = true;
	assert_eq!(marked(&ordered), (true, vec![true, false]));
	let body = ordered.input_body();
	assert_eq!(
		body["messages"][0]["content"],
		json!([{"type":"text","text":"Follow the task","cache_control":{"type":"ephemeral"}}])
	);
	let ModelContext::Ordered(parts) = &ordered.context else {
		panic!("expected an Ordered request");
	};
	assert_eq!(body["messages"][1]["content"][0]["text"], parts.stable);
	assert_eq!(body["messages"][1]["content"][1]["text"], parts.volatile);
}

#[rstest]
fn media_parts_after_the_volatile_part_are_never_marked(mut ordered: ModelRequest) {
	ordered.cache_breakpoints = true;
	ordered.content_parts = vec![ContentPart::Text("attachment".into())];
	assert_eq!(marked(&ordered), (true, vec![true, false, false]));
}

#[test]
fn without_history_the_breakpoint_still_ends_the_stable_part() {
	let mut request = request(
		RequestProjection::Ordered(CacheScope {
			tenant: "tenant-a".into(),
			key_version: 1,
		}),
		0,
	);
	request.cache_breakpoints = true;
	assert_eq!(marked(&request), (true, vec![true, false]));
}

#[rstest]
fn breakpoints_never_change_an_ordered_estimate(mut ordered: ModelRequest) {
	let unmarked = ordered.estimated_total_tokens();
	ordered.cache_breakpoints = true;
	assert_eq!(ordered.estimated_total_tokens(), unmarked);
	assert_eq!(
		unmarked,
		ordered.input_body().to_string().len()
			+ crate::projection::CACHE_SALT_LINE_RESERVE
			+ 64 + 1024
	);
}

#[rstest]
fn breakpoints_are_part_of_the_request_identity(mut ordered: ModelRequest) {
	let unmarked = ordered.inference_digest();
	assert!(
		!serde_json::to_string(&ordered)
			.unwrap()
			.contains("cache_breakpoints")
	);
	ordered.cache_breakpoints = true;
	assert_ne!(ordered.inference_digest(), unmarked);
}

#[test]
fn legacy_requests_reject_breakpoints_and_keep_their_bytes() {
	let mut legacy = request(RequestProjection::Legacy, 2);
	let (body, estimate) = (legacy.input_body(), legacy.estimated_total_tokens());
	legacy.cache_breakpoints = true;
	assert!(legacy.validate().is_err());
	assert_eq!(legacy.input_body(), body);
	assert_eq!(legacy.estimated_total_tokens(), estimate);
}
