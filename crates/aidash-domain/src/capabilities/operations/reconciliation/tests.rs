use super::*;
use crate::RunPhase;
use rstest::{fixture, rstest};
use serde_json::json;
#[fixture]
fn operation() -> Snapshot {
	Snapshot {
		id: Uuid::from_u128(1),
		area_id: Uuid::from_u128(2),
		run_id: Uuid::from_u128(3),
		tenant: "tenant".into(),
		principal: "owner".into(),
		credential_id: Uuid::from_u128(4),
		subjects: json!(["owner"]),
		digest: "digest".into(),
		kind: "shell".into(),
		state: "submitted".into(),
		epoch: 5,
		generation: 6,
		revision: 7,
		policy_revision: 8,
		input: json!({"command":"echo fixture","seconds":1}),
		result: json!({}),
		runner_instance: Some("runner".into()),
	}
}
#[fixture]
fn run() -> RunMetadata {
	RunMetadata {
		id: Uuid::from_u128(3),
		task_id: Uuid::from_u128(9),
		workspace_id: Uuid::from_u128(10),
		home_node: "aidash://local".into(),
		agent_id: "agent".into(),
		agent_version: "1".into(),
		phase: RunPhase::Ready,
		control: RunControl::Active,
		step: 0,
		revision: 1,
		observed_input_seq: 0,
		ledger_worker_ready: true,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: chrono::Utc::now(),
	}
}
#[fixture]
fn area() -> Area {
	Area {
		id: Uuid::from_u128(2),
		workspace_id: Uuid::from_u128(10),
		generation: 6,
		revision: 7,
		epoch: 5,
		manifest: json!([]),
	}
}
#[fixture]
fn limits() -> Limits {
	Limits {
		admission: true,
		working_bytes: 1024,
		output_bytes: 1024,
		read_bytes: 8,
	}
}
#[rstest]
#[case::prepared("prepared", true)]
#[case::submitted("submitted", true)]
#[case::running("running", true)]
#[case::cancelling("cancelling", true)]
#[case::completed("completed", false)]
#[case::withdrawn("withdrawn", false)]
#[case::unknown("other", false)]
fn reconciliation_only_drives_the_original_active_states(
	mut operation: Snapshot,
	#[case] state: &str,
	#[case] active: bool,
) {
	operation.state = state.into();
	assert_eq!(operation.active(), active);
}
#[rstest]
#[case::same("same", true)]
#[case::epoch("epoch", false)]
#[case::generation("generation", false)]
#[case::revision("revision", false)]
fn a_writer_must_match_every_area_fence(
	operation: Snapshot,
	mut area: Area,
	#[case] change: &str,
	#[case] allowed: bool,
) {
	match change {
		"epoch" => area.epoch += 1,
		"generation" => area.generation += 1,
		"revision" => area.revision += 1,
		_ => {}
	}
	assert_eq!(operation.writer_matches(&area), allowed);
}
#[rstest]
#[case::first_submission(None, false, false)]
#[case::same_runner(Some("runner"), false, false)]
#[case::new_runner(Some("old"), false, true)]
#[case::same_cancel(Some("runner"), true, false)]
#[case::unknown_cancel(None, true, true)]
fn cancellation_needs_positive_runner_identity_instead_of_first_submission_semantics(
	mut operation: Snapshot,
	#[case] previous: Option<&str>,
	#[case] cancelling: bool,
	#[case] changed: bool,
) {
	operation.runner_instance = previous.map(str::to_owned);
	assert_eq!(operation.runner_changed("runner", cancelling), changed);
}
#[rstest]
#[case::active("active", false)]
#[case::profile_disabled("disabled", true)]
#[case::cancel_request("operation", true)]
#[case::run_cancelled("control", true)]
#[case::run_terminal("terminal", true)]
fn narrowing_authority_switches_to_cancellation(
	mut operation: Snapshot,
	mut run: RunMetadata,
	mut limits: Limits,
	#[case] change: &str,
	#[case] cancelling: bool,
) {
	match change {
		"disabled" => limits.admission = false,
		"operation" => operation.state = "cancelling".into(),
		"control" => run.control = RunControl::Cancelled,
		"terminal" => run.phase = RunPhase::Completed,
		_ => {}
	}
	assert_eq!(operation.cancelling(limits, &run), cancelling);
}
