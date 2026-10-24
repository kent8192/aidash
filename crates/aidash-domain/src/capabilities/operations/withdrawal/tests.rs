use super::*;
use rstest::{fixture, rstest};
#[fixture]
fn snapshot() -> Snapshot {
	Snapshot {
		operation_id: Uuid::from_u128(1),
		kind: "shell".into(),
		state: "running".into(),
		result: json!({}),
		epoch: 8,
		generation: 3,
		area_epoch: 8,
		area_generation: 3,
		area_state: "running".into(),
	}
}
#[rstest]
#[case::prepared("prepared", true)]
#[case::submitted("submitted", true)]
#[case::running("running", true)]
#[case::cancelling("cancelling", true)]
#[case::completed("completed", false)]
#[case::failed("failed", false)]
#[case::cancelled("cancelled", false)]
#[case::withdrawn("withdrawn", false)]
#[case::uncertain("uncertain", false)]
fn withdrawal_touches_only_reconcilable_committed_operations(
	mut snapshot: Snapshot,
	#[case] state: &str,
	#[case] active: bool,
) {
	snapshot.state = state.into();
	assert_eq!(snapshot.active(), active);
}
#[rstest]
#[case::prepared("prepared", true, "withdrawn", false, "active")]
#[case::stopped("running", true, "withdrawn", true, "uncertain")]
#[case::pending("submitted", false, "cancelling", true, "uncertain")]
fn dispatched_writers_keep_possible_effects_even_after_a_confirmed_stop(
	mut snapshot: Snapshot,
	#[case] state: &str,
	#[case] stopped: bool,
	#[case] status: &str,
	#[case] effects: bool,
	#[case] area: &str,
) {
	snapshot.state = state.into();
	let change = snapshot.change(stopped);
	assert_eq!(change.state, status);
	assert_eq!(change.area_state, Some(area));
	let mut expected = json!({"termination_confirmed":stopped,"effects_may_have_occurred":effects,"error":"AUTHORITY_WITHDRAWN"});
	if effects {
		expected["runner_acknowledged"] = json!(false);
	}
	assert_eq!(change.result, expected);
}
#[rstest]
#[case::epoch("epoch")]
#[case::generation("generation")]
#[case::state("state")]
fn an_old_operation_cannot_change_a_newer_area(mut snapshot: Snapshot, #[case] change: &str) {
	match change {
		"epoch" => snapshot.area_epoch += 1,
		"generation" => snapshot.area_generation += 1,
		"state" => snapshot.area_state = "active".into(),
		_ => unreachable!(),
	}
	assert_eq!(snapshot.change(true).area_state, None);
}
/// A withdrawn Check keeps the Area fenced only while its writer may run: it
/// never exports `/work`, so a confirmed stop reopens the Area, including one
/// an earlier unconfirmed pass fenced.
#[rstest]
#[case::confirmed_running_area(true, "running", Some("active"))]
#[case::pending_stop(false, "running", Some("uncertain"))]
#[case::confirmed_after_a_fenced_pass(true, "uncertain", Some("active"))]
#[case::still_unconfirmed(false, "uncertain", None)]
fn a_withdrawn_check_reopens_the_area_once_its_writer_stopped(
	mut snapshot: Snapshot,
	#[case] stopped: bool,
	#[case] area: &str,
	#[case] expected: Option<&str>,
) {
	snapshot.kind = super::super::VERIFICATION.into();
	snapshot.state = "cancelling".into();
	snapshot.area_state = area.into();
	assert_eq!(snapshot.change(stopped).area_state, expected);
}
#[rstest]
fn a_withdrawn_shell_stays_fenced_after_a_confirmed_stop(mut snapshot: Snapshot) {
	snapshot.state = "cancelling".into();
	snapshot.area_state = "uncertain".into();
	assert_eq!(snapshot.change(true).area_state, None);
}
