//! The bounded Run representation used by list views.
use super::{LeaseWait, RecoveryState, RunInspection, RunMetadata, RunState, StateVersion};
use crate::context::ContextEvent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunSummary {
	pub state_version: StateVersion,
	#[serde(flatten)]
	pub metadata: RunMetadata,
	pub state: Option<RunState>,
	pub recovery: Option<RecoveryState>,
	pub state_error: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lease_wait: Option<LeaseWait>,
	pub input_tokens: Option<u64>,
	pub output_tokens: Option<u64>,
	pub tool_calls: Option<usize>,
}

impl From<RunInspection> for RunSummary {
	fn from(run: RunInspection) -> Self {
		let input_tokens = run
			.context
			.as_ref()
			.and_then(|context| context.usage.as_ref().map(|usage| usage.input_tokens));
		let output_tokens = run
			.context
			.as_ref()
			.and_then(|context| context.usage.as_ref().map(|usage| usage.output_tokens));
		let tool_calls = run.context.as_ref().map(|context| {
			context
				.history
				.iter()
				.filter(|entry| matches!(entry.event, ContextEvent::Tool { .. }))
				.count()
		});
		Self {
			state_version: run.state_version,
			metadata: run.metadata,
			state: run.state,
			recovery: run.recovery,
			state_error: run.state_error,
			lease_wait: run.lease_wait,
			input_tokens,
			output_tokens,
			tool_calls,
		}
	}
}

impl std::ops::Deref for RunSummary {
	type Target = RunMetadata;
	fn deref(&self) -> &Self::Target {
		&self.metadata
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		RunControl, RunPhase,
		context::{Context, ContextUsage},
	};
	use serde_json::json;
	use uuid::Uuid;

	#[test]
	fn summary_keeps_state_and_only_exposes_context_aggregates() {
		let mut context = Context {
			usage: Some(ContextUsage {
				input_tokens: 42,
				output_tokens: 17,
				context_window: 4096,
				compactions: 0,
				exposure: None,
			}),
			..Context::default()
		};
		for id in ["one", "two"] {
			context.push(ContextEvent::Tool {
				call: crate::provider::ToolCall {
					id: id.into(),
					name: "tool".into(),
					arguments: json!({}),
				},
				result: json!({"secret": "not in a list"}),
			});
		}
		let run = RunInspection {
			state_version: StateVersion::default(),
			metadata: RunMetadata {
				id: Uuid::from_u128(1),
				task_id: Uuid::from_u128(2),
				workspace_id: Uuid::from_u128(3),
				node: "brave-otter-k3m9q2xa".parse().unwrap(),
				home_node: "aidash://local/brave-otter-k3m9q2xa".into(),
				agent_id: "agent".into(),
				agent_version: "1".into(),
				phase: RunPhase::Ready,
				control: RunControl::Active,
				control_revision: 0,
				control_source: Default::default(),
				step: 0,
				revision: 0,
				observed_input_seq: 0,
				ledger_worker_ready: true,
				error: None,
				lease_owner: None,
				lease_until: None,
				updated_at: chrono::Utc::now(),
			},
			state: Some(RunState::default()),
			recovery: Some(RecoveryState::default()),
			context: Some(context),
			state_error: None,
			lease_wait: None,
			declinable_call: None,
		};
		let full = serde_json::to_value(&run).unwrap();
		let summary = serde_json::to_value(RunSummary::from(run)).unwrap();
		assert_eq!(summary["id"], full["id"]);
		assert_eq!(summary["state"], full["state"]);
		assert_eq!(summary["recovery"], full["recovery"]);
		assert_eq!(summary["input_tokens"], 42);
		assert_eq!(summary["output_tokens"], 17);
		assert_eq!(summary["tool_calls"], 2);
		assert!(summary.get("context").is_none());
		assert!(full["context"].to_string().contains("not in a list"));
	}
}
