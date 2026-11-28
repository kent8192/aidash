use super::*;
use crate::transactions::Mutation;
use rstest::{fixture, rstest};
use serde_json::json;

#[fixture]
fn task() -> Task {
	serde_json::from_value(json!({"id":Uuid::from_u128(1),"thread_id":Uuid::from_u128(2),"title":"Task","description":"Complete atomically","status":"RUNNING","requirements":{},"owner":"aidash://worker/agents/agent@1","created_by":"deployment_administrator","dependencies":[],"parent_id":null,"revision":3,"created_at":"2030-01-01T00:00:00Z"})).unwrap()
}

#[fixture]
fn run() -> RunMetadata {
	serde_json::from_value(json!({"id":Uuid::from_u128(4),"task_id":Uuid::from_u128(1),"thread_id":Uuid::from_u128(2),"node":"brave-otter-k3m9q2xa","home_node":"aidash://home/brave-otter-k3m9q2xa","agent_id":"agent","agent_version":"1","phase":"TOOL_CALL","control":"ACTIVE","step":7,"revision":5,"observed_input_seq":0,"ledger_worker_ready":false,"error":null,"lease_owner":null,"lease_until":null,"updated_at":"2030-01-01T00:00:00Z"})).unwrap()
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

/// A share whose only participants are a home Node and a worker Node, sealed
/// over `worker`'s mutations; the share is the home Node's.
fn share(worker: Vec<Mutation>, home: Vec<Mutation>) -> crate::transactions::share::Share {
	let manifest = crate::transactions::Manifest {
		id: Uuid::from_u128(9),
		coordinator: "aidash://home".into(),
		isolation: crate::transactions::Isolation::Serializable,
		deadline: "2030-01-01T00:00:00Z".parse().unwrap(),
		participants: vec![
			crate::transactions::Participant {
				node_id: "aidash://home".into(),
				mutations: home,
			},
			crate::transactions::Participant {
				node_id: "aidash://worker".into(),
				mutations: worker,
			},
		],
	};
	manifest.share(&"0".repeat(64), "aidash://home").unwrap()
}

#[rstest]
fn paired_completion_is_bound_to_the_same_task_and_participant() {
	let task = Uuid::from_u128(1);
	let other = Uuid::from_u128(2);
	let finish = |run: u128, task_id: Uuid| Mutation::FinishRun {
		run_id: Uuid::from_u128(run),
		task_id,
		expected_revision: 0,
	};
	let complete = Mutation::CompleteTask {
		task_id: task,
		expected_revision: 0,
		text: "Done".into(),
	};
	// The home Node sees the worker's claims, never its mutations.
	let only_other = share(vec![finish(3, other)], vec![]);
	assert_eq!(
		validate_delegated_completion(&only_other, "aidash://worker", task),
		Err(Error::Invalid(
			"delegated completion requires its participant's execution finalization".into()
		))
	);
	let paired = share(vec![finish(3, other), finish(4, task)], vec![]);
	assert_eq!(
		validate_delegated_completion(&paired, "aidash://worker", task),
		Ok(())
	);
	// A finalization on the home Node is no claim of the worker's.
	assert_eq!(
		validate_delegated_completion(
			&share(vec![], vec![finish(4, task)]),
			"aidash://worker",
			task
		),
		Err(Error::Invalid(
			"delegated completion requires its participant's execution finalization".into()
		))
	);
	assert_eq!(
		validate_home_completion(&paired, "aidash://home", task),
		Err(Error::Invalid(
			"execution finalization requires the home task's atomic completion".into()
		))
	);
	assert_eq!(
		validate_home_completion(&share(vec![], vec![complete]), "aidash://home", task),
		Ok(())
	);
}
