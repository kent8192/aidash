//! Deterministic Tool Batch planning. A batch is the longest contiguous run of
//! calls, in the model's order, whose providers declare Concurrency safety and
//! derive non-conflicting Resource Claims with bounded output.
use super::TOOL_EVENT_RESERVE;
use crate::ports::execution::Tools;
use aidash_domain::{
	context::{self, Context, ContextEvent},
	provider::ToolCall,
	tool::concurrency::{ConcurrentCall, RESULT_ENVELOPE_BYTES, batchable},
};
use serde_json::json;

/// The request budget a batch's worst-case results must fit, as in result fitting.
#[derive(Clone, Copy)]
pub struct BatchBudget {
	pub request_tokens: usize,
	pub request_window: usize,
}

/// Exclusive end of the Tool Batch that starts at `cursor`. A batch shorter
/// than two calls is not a batch: its first call takes the sequential path.
pub fn plan_tool_batch(
	context: &Context,
	calls: &[ToolCall],
	cursor: usize,
	tools: &Tools,
	ceiling: usize,
	budget: BatchBudget,
) -> usize {
	let mut admitted: Vec<ConcurrentCall> = Vec::new();
	let mut probe = context.clone();
	let mut tokens = budget.request_tokens;
	for (index, call) in calls.iter().enumerate().skip(cursor) {
		if admitted.len() >= ceiling {
			break;
		}
		let Some(tool) = tools.get(&call.name) else {
			break;
		};
		if !batchable(&tool.contract().behavior) {
			break;
		}
		let Some(concurrent) = tool.concurrent_call(&call.arguments) else {
			break;
		};
		if admitted.iter().any(|held| held.conflicts(&concurrent)) {
			break;
		}
		// Measure the worst encoding of the bounded result in the request
		// estimator: every byte a control character, escaped at each level.
		let event = ContextEvent::tool(
			call.clone(),
			json!({"bound": "\u{1}".repeat(concurrent.output_bytes + RESULT_ENVELOPE_BYTES)}),
		);
		let growth = context::tool_event_growth(&probe, &event);
		let remaining = calls.len() - (index + 1);
		let maximum = budget
			.request_window
			.saturating_sub(remaining.saturating_mul(TOOL_EVENT_RESERVE));
		if tokens.saturating_add(growth) > maximum {
			break;
		}
		tokens = tokens.saturating_add(growth);
		probe.history.push(event);
		admitted.push(concurrent);
	}
	if admitted.len() < 2 {
		cursor
	} else {
		cursor + admitted.len()
	}
}

#[cfg(test)]
mod tests;
