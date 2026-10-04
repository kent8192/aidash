use super::*;
use crate::ArtifactInput;
use rstest::{fixture, rstest};
use serde_json::json;

#[fixture]
fn task() -> Task {
	serde_json::from_value(json!({"id":Uuid::from_u128(1),"workspace_id":Uuid::from_u128(2),"title":"Task","description":"Complete atomically","status":"RUNNING","requirements":{},"owner":"aidash://worker/agents/agent@1","created_by":"operator","dependencies":[],"parent_id":null,"revision":3,"created_at":"2030-01-01T00:00:00Z"})).unwrap()
}

#[fixture]
fn run() -> RunMetadata {
	serde_json::from_value(json!({"id":Uuid::from_u128(4),"task_id":Uuid::from_u128(1),"workspace_id":Uuid::from_u128(2),"home_node":"aidash://home","agent_id":"agent","agent_version":"1","phase":"TOOL_CALL","control":"ACTIVE","step":7,"revision":5,"observed_input_seq":0,"ledger_worker_ready":false,"error":null,"lease_owner":null,"lease_until":null,"updated_at":"2030-01-01T00:00:00Z"})).unwrap()
}

#[rstest]
#[case::ready(TaskStatus::Running, 3, true, true)]
#[case::unowned(TaskStatus::Running, 3, false, false)]
#[case::stale_revision(TaskStatus::Running, 2, true, false)]
#[case::claimed(TaskStatus::Claimed, 3, true, false)]
#[case::completed(TaskStatus::Completed, 3, true, false)]
#[case::cancelled(TaskStatus::Cancelled, 3, true, false)]
fn task_completion_requires_the_running_owner_and_exact_revision(
	mut task: Task,
	#[case] status: TaskStatus,
	#[case] revision: i64,
	#[case] owned: bool,
	#[case] accepted: bool,
) {
	task.status = status;
	if !owned {
		task.owner = None;
	}
	assert_eq!(
		validate_task(&task, revision),
		if accepted {
			Ok(())
		} else {
			Err(Error::Conflict(
				"task must be running at its expected revision".into(),
			))
		}
	);
}

#[rstest]
#[case::ready(RunPhase::ToolCall, RunControl::Active, false, 5, 1, true)]
#[case::paused(RunPhase::ToolCall, RunControl::Paused, false, 5, 1, true)]
#[case::leased(RunPhase::ToolCall, RunControl::Active, true, 5, 1, false)]
#[case::cancelled(RunPhase::ToolCall, RunControl::Cancelled, false, 5, 1, false)]
#[case::thinking(RunPhase::Thinking, RunControl::Active, false, 5, 1, false)]
#[case::stale_revision(RunPhase::ToolCall, RunControl::Active, false, 4, 1, false)]
#[case::another_task(RunPhase::ToolCall, RunControl::Active, false, 5, 9, false)]
fn execution_finalization_requires_a_quiescent_matching_tool_call(
	mut run: RunMetadata,
	#[case] phase: RunPhase,
	#[case] control: RunControl,
	#[case] leased: bool,
	#[case] revision: i64,
	#[case] task: u128,
	#[case] accepted: bool,
) {
	run.phase = phase;
	run.control = control;
	assert_eq!(
		validate_run(&run, leased, Uuid::from_u128(task), revision),
		if accepted {
			Ok(())
		} else {
			Err(Error::Conflict(
				"run must be quiescent at its expected tool-call revision".into(),
			))
		}
	);
}

#[rstest]
fn paired_completion_is_bound_to_the_same_task_and_participant() {
	let task = Uuid::from_u128(1);
	let other = Uuid::from_u128(2);
	let mut participant = Participant {
		node_id: "aidash://worker".into(),
		mutations: vec![Mutation::FinishRun {
			run_id: Uuid::from_u128(3),
			task_id: other,
			expected_revision: 0,
		}],
	};
	assert_eq!(
		validate_delegated_completion(&participant, task),
		Err(Error::Invalid(
			"delegated completion requires its participant's execution finalization".into()
		))
	);
	participant.mutations.push(Mutation::FinishRun {
		run_id: Uuid::from_u128(4),
		task_id: task,
		expected_revision: 0,
	});
	assert_eq!(validate_delegated_completion(&participant, task), Ok(()));
	assert_eq!(
		validate_home_completion(&participant, task),
		Err(Error::Invalid(
			"execution finalization requires the home task's atomic completion".into()
		))
	);
	participant.mutations.push(Mutation::CompleteTask {
		task_id: task,
		expected_revision: 0,
		artifact: ArtifactInput {
			kind: "text".into(),
			name: "Answer".into(),
			content: json!("Done"),
		},
	});
	assert_eq!(validate_home_completion(&participant, task), Ok(()));
}
