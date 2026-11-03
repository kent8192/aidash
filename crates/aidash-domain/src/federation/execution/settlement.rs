//! Saved local evidence narrows post-cancellation Home callbacks to settlement.
use crate::{RunControl, RunPhase, TaskStatus, run_input::RunInput, run_state::RawRun};
use serde_json::{Value, json};

/// These saved continuations have already chosen their outcome. Stop owners
/// must deliver them before replacing pending execution with a terminal state.
pub fn has_saved_continuation(raw: &RawRun) -> bool {
	crate::run_state::decode(raw.metadata.phase, raw.pending.clone()).is_ok_and(|(state, _)| {
		state.failure_delivery() || state.tool().is_ok_and(|tool| tool.finalizing)
	})
}

/// This does not replace Home authorization. The remote owner still checks its
/// original grant, current policy and revision before applying a callback.
pub fn allows(
	raw: &RawRun,
	inputs: &[RunInput],
	result_name: &str,
	operation: &str,
	data: &Value,
) -> bool {
	let decoded = crate::run_state::decode(raw.metadata.phase, raw.pending.clone()).ok();
	let finalizing = decoded
		.as_ref()
		.and_then(|(state, _)| state.tool().ok())
		.filter(|tool| tool.finalizing);
	let saved_failure = decoded.as_ref().and_then(|(state, _)| match state {
		crate::RunState::Waiting(wait) => match wait.as_ref() {
			crate::WaitingState::FailureDelivery { target, .. } => Some(target.task_status()),
			_ => None,
		},
		_ => None,
	});
	let terminal = match raw.metadata.phase {
		RunPhase::Completed => Some(TaskStatus::Completed),
		RunPhase::Failed => Some(TaskStatus::Failed),
		RunPhase::Cancelled => Some(TaskStatus::Cancelled),
		_ if saved_failure.is_some() => saved_failure,
		_ if raw.metadata.control == RunControl::Cancelled => Some(TaskStatus::Cancelled),
		_ => None,
	};
	let keys_only = |keys: &[&str]| {
		data.as_object().is_some_and(|object| {
			object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key))
		})
	};
	let original_run = data["run_id"] == json!(raw.metadata.id);
	let input = inputs.iter().find(|input| {
		data["key"].as_str() == Some(input.idempotency_key.as_str())
			&& data["content"].as_str() == Some(input.content.as_str())
	});
	let exact_input = original_run && input.is_some();
	match operation {
		"run_message_commit" => {
			exact_input
				&& keys_only(&["run_id", "key", "content", "input_seq"])
				&& data["input_seq"].as_i64() == input.map(|input| input.seq)
		}
		"run_message_reserve" | "run_message_delivery" => {
			exact_input && keys_only(&["run_id", "key", "content"])
		}
		"run_message_release" | "run_message_ack" => {
			original_run
				&& keys_only(&["run_id", "keys"])
				&& data["keys"].as_array().is_some_and(|keys| {
					!keys.is_empty()
						&& keys.iter().all(|key| {
							inputs.iter().any(|input| {
								key.as_str() == Some(input.idempotency_key.as_str())
									&& (operation == "run_message_release"
										|| input.seq <= raw.metadata.observed_input_seq)
							})
						})
				})
		}
		"run_message_terminal_transition" => {
			original_run
				&& keys_only(&["revision", "status", "run_id", "keys", "through_seq"])
				&& terminal.is_some_and(|target| data["status"] == json!(target))
				&& data["keys"] == json!([])
				&& data["through_seq"].as_i64()
					== Some(inputs.iter().map(|input| input.seq).max().unwrap_or(0))
				&& data["revision"].as_i64().is_some()
		}
		"run_message_complete" => {
			original_run
				&& keys_only(&["run_id", "through_seq", "key", "artifact"])
				&& finalizing.is_some_and(|tool| {
					data["key"] == json!(format!("{}:complete", raw.metadata.id))
						&& data["through_seq"].as_i64() == Some(tool.included_input_seq)
						&& data["artifact"]
							== json!({"kind":"text","name":result_name,"content":tool.response.text})
				})
		}
		// Home::transition_terminal/complete read only their original bound Task.
		"task" => keys_only(&[]) && (terminal.is_some() || finalizing.is_some()),
		"run_message_delivery_capability" => {
			keys_only(&[]) && (!inputs.is_empty() || terminal.is_some() || finalizing.is_some())
		}
		_ => false,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::run_state::{FailureTarget, RecoveryState, RunState, WaitingState};
	use uuid::Uuid;

	#[rstest::rstest]
	#[case::saved_failure(TaskStatus::Failed, true)]
	#[case::cancellation(TaskStatus::Cancelled, false)]
	#[case::invented_success(TaskStatus::Completed, false)]
	fn saved_failure_delivery_survives_cancel_without_admitting_fresh_commands(
		#[case] target: TaskStatus,
		#[case] allowed: bool,
	) {
		let run = Uuid::from_u128(1);
		let pending = crate::run_state::encode(
			&RunState::Waiting(Box::new(WaitingState::FailureDelivery {
				target: FailureTarget::Failed,
				wake_at: "2030-01-01T00:00:00Z".parse().unwrap(),
				last_delivery_error: Some("retry delivery".into()),
			})),
			&RecoveryState::default(),
		)
		.unwrap();
		let raw: RawRun = serde_json::from_value(json!({
			"id":run,"task_id":Uuid::from_u128(2),"workspace_id":Uuid::from_u128(3),
			"node":"brave-otter-k3m9q2xa","home_node":"aidash://home/brave-otter-k3m9q2xa",
			"agent_id":"producer","agent_version":"1","phase":"WAITING","control":"CANCELLED",
			"step":3,"revision":7,"observed_input_seq":0,"ledger_worker_ready":true,"error":"saved failure",
			"lease_owner":null,"lease_until":null,"updated_at":"2030-01-01T00:00:00Z",
			"context":crate::context::Context::default(),"pending":pending
		}))
		.unwrap();
		let mut data = json!({"revision":4,"status":target,"run_id":run,"keys":[],"through_seq":0});
		assert_eq!(
			allows(
				&raw,
				&[],
				"result",
				"run_message_terminal_transition",
				&data
			),
			allowed
		);
		data["run_id"] = json!(Uuid::from_u128(9));
		assert!(!allows(
			&raw,
			&[],
			"result",
			"run_message_terminal_transition",
			&data
		));
		assert!(!allows(
			&raw,
			&[],
			"result",
			"message",
			&json!({"content":"new work"})
		));
	}
}
