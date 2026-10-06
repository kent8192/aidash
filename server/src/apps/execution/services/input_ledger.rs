//! Admission and context-budget rules, independent of persistence.
use crate::apps::execution::serializers::run_inputs::RunInput;
use crate::{Error, Result};
use serde_json::json;
use uuid::Uuid;

pub(crate) fn input_size(sender: &str, content: &str) -> usize {
	// Match the conservative byte estimate of the JSON provider context.
	serde_json::to_string(&json!({"seq":i64::MAX,"sender":sender,"content":content}))
		.map_or(usize::MAX, |value| crate::context::estimated_tokens(&value))
}

pub(crate) fn reference_size(sender: &str, message: Uuid) -> usize {
	serde_json::to_string(&json!({"seq":i64::MAX,"sender":sender,"record":{"kind":"message","id":message},"requires_workspace_read":true}))
		.map_or(usize::MAX, |value| crate::context::estimated_tokens(&value))
}

pub(crate) fn context_size(input: &RunInput) -> usize {
	if input.reference_only {
		input
			.message_id
			.map_or(usize::MAX, |id| reference_size(&input.sender, id))
	} else {
		input_size(&input.sender, &input.content)
	}
}

pub(crate) fn closing(run: &crate::domain::Run) -> bool {
	use crate::domain::{RunControl, RunState};
	run.phase().is_terminal()
		|| run.control == RunControl::Cancelled
		|| matches!(&run.state, RunState::ToolCall(state) if state.finalizing)
		|| run.state.failure_delivery()
}

pub(crate) fn require_admission(
	run: &crate::domain::Run,
	task_terminal: bool,
	ledger_ready: bool,
	leased: bool,
) -> Result<()> {
	if task_terminal || closing(run) {
		return Err(Error::Conflict(
			"run message was not accepted because the run is completing or terminal".into(),
		));
	}
	if !ledger_ready && leased {
		return Err(Error::Conflict(
			"run is leased by a worker without input ledger fencing".into(),
		));
	}
	Ok(())
}
