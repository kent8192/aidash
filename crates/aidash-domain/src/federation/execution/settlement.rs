//! Saved local evidence narrows post-cancellation Home callbacks to settlement.
use crate::{RunControl, RunPhase, TaskStatus, run_input::RunInput, run_state::RawRun};
use serde_json::{Value, json};

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
	let terminal = match raw.metadata.phase {
		RunPhase::Completed => Some(TaskStatus::Completed),
		RunPhase::Failed => Some(TaskStatus::Failed),
		RunPhase::Cancelled => Some(TaskStatus::Cancelled),
		_ if raw.metadata.control == RunControl::Cancelled => Some(TaskStatus::Cancelled),
		_ => decoded.as_ref().and_then(|(state, _)| match state {
			crate::RunState::Waiting(wait) => match wait.as_ref() {
				crate::WaitingState::FailureDelivery { target, .. } => Some(target.task_status()),
				_ => None,
			},
			_ => None,
		}),
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
