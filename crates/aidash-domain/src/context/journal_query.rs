//! Offload exposure and durable Journal read counts.
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Context;
use crate::provider::ToolCall;

/// Stream 1 sets this marker when it first offloads a result.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Offloaded {}

pub fn offloaded_any(context: &Context) -> bool {
	context.offloaded.is_some()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct ReadCounts(Vec<ReadCount>);

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadCount {
	from: u64,
	through: u64,
	count: u64,
}

impl ReadCounts {
	pub fn is_empty(&self) -> bool {
		self.0.is_empty()
	}

	pub fn count(&self, range: (u64, u64)) -> u64 {
		self.0
			.iter()
			.find(|entry| (entry.from, entry.through) == range)
			.map_or(0, |entry| entry.count)
	}

	pub fn record(&mut self, range: (u64, u64)) {
		if let Some(entry) = self
			.0
			.iter_mut()
			.find(|entry| (entry.from, entry.through) == range)
		{
			entry.count = entry.count.saturating_add(1);
		} else {
			self.0.push(ReadCount {
				from: range.0,
				through: range.1,
				count: 1,
			});
		}
	}

	/// Count only a completed read, never a refusal or SESSION_RESET.
	pub fn record_result(&mut self, call: &ToolCall, result: &Value) {
		let input = match call.name.as_str() {
			"journal_query" if result["contract"] == "aidash.journal.query/1" => {
				call.arguments.get("journal_range").unwrap_or(&Value::Null)
			}
			"journal_read" if result["text"].is_string() => &call.arguments,
			"journal_search" if result["results"].is_array() => &call.arguments,
			_ => return,
		};
		let from = input.get("from_seq").and_then(Value::as_u64).unwrap_or(0);
		let through = input
			.get("through_seq")
			.and_then(Value::as_u64)
			.unwrap_or(i64::MAX as u64);
		if from <= through && through <= i64::MAX as u64 {
			self.record((from, through));
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn offload_marker_and_read_counts_survive_context_serialization() {
		let mut context = Context::default();
		assert!(!offloaded_any(&context));
		context.offloaded = Some(Offloaded {});
		let call = ToolCall {
			id: "1".into(),
			name: "journal_read".into(),
			arguments: json!({"from_seq":2,"through_seq":4}),
		};
		context.push_invocation(
			super::super::ContextEvent::tool(call.clone(), json!({"text":"line"})),
			"one",
		);
		context.push_invocation(
			super::super::ContextEvent::tool(call, json!({"error":"refused"})),
			"two",
		);
		let decoded: Context =
			serde_json::from_value(serde_json::to_value(context).unwrap()).unwrap();
		assert!(offloaded_any(&decoded));
		assert_eq!(decoded.journal_reads.count((2, 4)), 1);
	}

	#[test]
	fn query_reset_is_not_a_read_and_unbounded_query_counts_full_range() {
		let mut counts = ReadCounts::default();
		let call = ToolCall {
			id: "1".into(),
			name: "journal_query".into(),
			arguments: json!({"code":"journal()"}),
		};
		counts.record_result(&call, &json!({"session_reset":true}));
		assert!(counts.is_empty());
		counts.record_result(
			&call,
			&json!({"contract":"aidash.journal.query/1","stdout":""}),
		);
		assert_eq!(counts.count((0, i64::MAX as u64)), 1);
		counts.record_result(
			&call,
			&json!({"contract":"aidash.journal.query/1","stdout":""}),
		);
		assert_eq!(counts.count((0, i64::MAX as u64)), 2);
		assert_eq!(counts.count((1, 2)), 0);
	}
}
