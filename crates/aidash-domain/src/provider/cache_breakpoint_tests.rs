//! Request accounting and identity with explicit cache breakpoints (ADR 0019).
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
	let mut context = Context::default();
	for n in 1..=events {
		context.push(ContextEvent::tool(
			ToolCall {
				id: format!("call-{n}"),
				name: "workspace_read".into(),
				arguments: json!({"kind":"task"}),
			},
			json!({"read":n}),
		));
	}
	let pinned = json!({"identity":{"agent_id":"agent"},"task":{"title":"Task"},"agent_state":{"step":events}});
	RequestBudget {
		input_format: Default::default(),
		window: usize::MAX,
		instructions: "Follow the task",
		tools: &tools(),
		max_output_tokens: 64,
		projection: &projection,
		estimate: Default::default(),
	}
	.request(&context, &pinned)
}

#[fixture]
fn native() -> ModelRequest {
	request(
		RequestProjection::Native(CacheScope {
			tenant: "tenant-a".into(),
			key_version: 1,
		}),
		2,
	)
}

#[rstest]
fn breakpoints_never_change_an_native_estimate(mut native: ModelRequest) {
	let unmarked = native.estimated_total_tokens();
	native.cache_breakpoints = true;
	assert_eq!(native.estimated_total_tokens(), unmarked);
	assert_eq!(
		unmarked,
		native.input_body().to_string().len()
			+ crate::projection::CACHE_SALT_LINE_RESERVE
			+ 64 + 1024
	);
}

#[rstest]
fn breakpoints_are_part_of_the_request_identity(mut native: ModelRequest) {
	let unmarked = native.inference_digest();
	assert!(
		!serde_json::to_string(&native)
			.unwrap()
			.contains("cache_breakpoints")
	);
	native.cache_breakpoints = true;
	assert_ne!(native.inference_digest(), unmarked);
}

#[test]
fn standalone_requests_reject_breakpoints_and_keep_their_bytes() {
	let mut standalone = ModelRequest {
		context: ModelContext::native_data(&json!({"task":"standalone"})),
		..Default::default()
	};
	let (body, estimate) = (standalone.input_body(), standalone.estimated_total_tokens());
	standalone.cache_breakpoints = true;
	assert!(standalone.validate().is_err());
	assert_eq!(standalone.input_body(), body);
	assert_eq!(standalone.estimated_total_tokens(), estimate);
}

/// Each Tenant's salt starts a prefix no other Tenant can reuse, so identical
/// instructions and tools in two Tenants have distinct, undisclosed identities.
#[rstest]
#[case::tenant(false)]
#[case::input_format(true)]
fn cache_identity_follows_the_provider_cache_boundary(native: ModelRequest, #[case] format: bool) {
	// Arrange
	let mut other = request(
		RequestProjection::Native(CacheScope {
			tenant: if format { "tenant-a" } else { "tenant-b" }.into(),
			key_version: 1,
		}),
		2,
	);
	if format {
		other.input_format = InputFormat::Responses;
	}
	// Act
	let (a, b) = (
		native.cache_identity().unwrap(),
		other.cache_identity().unwrap(),
	);
	// Assert
	assert_ne!(a, b);
	assert!(!a.contains("tenant-a") && !b.contains("tenant-b"));
}
