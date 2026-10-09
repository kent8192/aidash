use super::*;
use crate::{Result, ports::execution::ExecutionTool};
use aidash_domain::{
	Run,
	provider::ToolSpec,
	tool::{
		ToolContract, builtin_contract,
		concurrency::{Resource, ResourceClaim},
	},
};
use async_trait::async_trait;
use rstest::rstest;
use serde_json::Value;
use std::sync::Arc;

struct Fake {
	contract: ToolContract,
	claim: Option<ResourceClaim>,
	output_bytes: usize,
}
#[async_trait]
impl ExecutionTool for Fake {
	fn specification(&self) -> ToolSpec {
		unreachable!("planning never reads specifications")
	}
	fn contract(&self) -> ToolContract {
		self.contract.clone()
	}
	fn replay_safe(&self) -> bool {
		self.contract.replay_safe()
	}
	async fn invoke(&self, _: &Run, _: Value, _: &str) -> Result<Value> {
		unreachable!("planning never dispatches")
	}
	fn concurrent_call(&self, _: &Value) -> Option<ConcurrentCall> {
		self.claim.map(|claim| ConcurrentCall {
			claims: vec![claim],
			output_bytes: self.output_bytes,
		})
	}
}

fn tools() -> Tools {
	let mut tools = Tools::new();
	let mut add = |name: &str, operation: &str, claim| {
		tools.insert(
			name.into(),
			Arc::new(Fake {
				contract: builtin_contract(operation).unwrap(),
				claim,
				output_bytes: 64,
			}) as Arc<dyn ExecutionTool>,
		);
	};
	add(
		"read",
		"file_read",
		Some(ResourceClaim::shared(Resource::WorkingArea)),
	);
	add(
		"list",
		"skill_list",
		Some(ResourceClaim::shared(Resource::PinnedSkills)),
	);
	add(
		"load",
		"skill_load",
		Some(ResourceClaim::exclusive(Resource::PinnedSkills)),
	);
	add("media", "file_read", None);
	add(
		"patch",
		"apply_patch",
		Some(ResourceClaim::shared(Resource::WorkingArea)),
	);
	add(
		"message",
		"workspace_read",
		Some(ResourceClaim::shared(Resource::WorkingArea)),
	);
	tools
}

fn calls(names: &[&str]) -> Vec<ToolCall> {
	names
		.iter()
		.enumerate()
		.map(|(index, name)| ToolCall {
			id: format!("call-{index}"),
			name: (*name).into(),
			arguments: json!({}),
		})
		.collect()
}

const ROOMY: BatchBudget = BatchBudget {
	request_tokens: 0,
	request_window: 1_000_000,
};

#[rstest]
#[case(&["read", "read", "list"], 0, 8, 3)]
#[case(&["read", "read", "read"], 0, 2, 2)]
#[case(&["read", "read"], 0, 1, 0)]
#[case(&["patch", "read", "read", "read"], 0, 8, 0)]
#[case(&["patch", "read", "read", "read"], 1, 8, 4)]
#[case(&["read", "patch", "read", "read"], 0, 8, 0)]
#[case(&["read", "read", "patch", "read", "read"], 0, 8, 2)]
#[case(&["read", "load", "list", "read"], 0, 8, 2)]
#[case(&["load", "load"], 0, 8, 0)]
#[case(&["read", "media", "read"], 0, 8, 0)]
#[case(&["read", "message", "read"], 0, 8, 0)]
#[case(&["read", "absent", "read"], 0, 8, 0)]
fn batches_are_contiguous_conflict_free_and_within_the_ceiling(
	#[case] names: &[&str],
	#[case] cursor: usize,
	#[case] ceiling: usize,
	#[case] end: usize,
) {
	let expected = if end == 0 { cursor } else { end };
	let calls = calls(names);
	let tools = tools();
	let planned = plan_tool_batch(&Context::default(), &calls, cursor, &tools, ceiling, ROOMY);
	assert_eq!(planned, expected);
	// Planning is a pure function of the same durable inputs.
	assert_eq!(
		plan_tool_batch(&Context::default(), &calls, cursor, &tools, ceiling, ROOMY),
		planned
	);
}

#[test]
fn worst_case_result_encoding_must_fit_beside_later_call_reserves() {
	let calls = calls(&["read", "read", "read", "patch"]);
	let tools = tools();
	let planned = |request_tokens, request_window| {
		plan_tool_batch(
			&Context::default(),
			&calls,
			0,
			&tools,
			8,
			BatchBudget {
				request_tokens,
				request_window,
			},
		)
	};
	// Every bounded byte as an escaped control character, as the planner probes.
	let one = context::tool_event_growth(
		&Context::default(),
		&ContextEvent::tool(
			calls[0].clone(),
			json!({"bound": "\u{1}".repeat(64 + RESULT_ENVELOPE_BYTES)}),
		),
	);
	assert!(one > 7 * (64 + RESULT_ENVELOPE_BYTES));
	let reserve = TOOL_EVENT_RESERVE;
	assert_eq!(planned(0, 2 * one + 2 * reserve - 16), 0);
	assert_eq!(planned(0, 2 * one + 2 * reserve + 16), 2);
	assert_eq!(planned(0, 3 * one + reserve + 16), 3);
	assert_eq!(planned(one, 3 * one + 2 * reserve + 16), 2);
}
