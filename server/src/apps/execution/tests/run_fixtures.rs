use aidash_server::apps::execution::models::{
	Run,
	states::{RunControl, RunPhase},
};
use chrono::Utc;
use rstest::fixture;
use serde_json::json;
use uuid::Uuid;

#[fixture]
pub fn run() -> Run {
	Run {
		node: aidash_domain::nodes::NodeName::derived("acme")
			.as_str()
			.parse()
			.unwrap(),
		id: Uuid::new_v4(),
		tenant: "acme".into(),
		task_id: Uuid::new_v4(),
		workspace_id: Uuid::new_v4(),
		home_node: "aidash://remote-delivery".into(),
		agent_id: "fixture".into(),
		agent_version: "1.0.0".into(),
		phase: RunPhase::Ready,
		control: RunControl::Active,
		context: json!(aidash_server::context::Context::default()).into(),
		pending: pending(
			aidash_server::domain::RunState::default(),
			Default::default(),
		)
		.into(),
		step: 0,
		revision: 0,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: Utc::now(),
		observed_input_seq: 0,
		ledger_worker_ready: true,
		control_revision: 0,
		control_source: Default::default(),
		semantic_retry_cycle: 0,
		pending_human_request_id: None,
	}
}

/// Encode complete native state fixtures through the production boundary.
pub fn pending(
	state: aidash_server::domain::RunState,
	recovery: aidash_server::domain::RecoveryState,
) -> serde_json::Value {
	json!({"state_version":1,"data":json!(state)["data"],"recovery":recovery})
}

#[allow(
	dead_code,
	reason = "Shared fixture targets independently use different Run constructors"
)]
pub fn human_pending(request_id: Uuid) -> serde_json::Value {
	use aidash_server::domain::{ReadyState, ResumeState, RunState, WaitingState};
	pending(
		RunState::Waiting(Box::new(WaitingState::Human {
			request_id,
			answer_by: None,
			resume: ResumeState::Ready(ReadyState {}),
		})),
		Default::default(),
	)
}

/// A Run waiting on an approval ADR 0075 leaves outside the local request table:
/// `home` for the human request a Run executing for another Node sent to that
/// Node, otherwise a capability approval record.
#[allow(
	dead_code,
	reason = "Shared fixture targets independently use different Run constructors"
)]
pub fn approval_pending(approval_id: Uuid, home: bool) -> serde_json::Value {
	use aidash_domain::run_state::{ApprovalRef, ToolCallState};
	use aidash_server::domain::{RunState, WaitingState};
	let approval = if home {
		ApprovalRef::Home {
			request_id: approval_id,
			key: "run:0:0".into(),
			call: aidash_server::provider::ToolCall {
				id: "call".into(),
				name: "plugin_0".into(),
				arguments: json!({}),
			},
			expires_at: Utc::now() + chrono::Duration::minutes(15),
		}
	} else {
		ApprovalRef::Capability { approval_id }
	};
	pending(
		RunState::Waiting(Box::new(WaitingState::Approval {
			approval,
			resume: Box::new(ToolCallState::default()),
		})),
		Default::default(),
	)
}
