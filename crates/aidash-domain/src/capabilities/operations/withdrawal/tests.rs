use super::*;
use rstest::{fixture, rstest};
#[fixture]
fn snapshot() -> Snapshot {
	Snapshot {
		operation_id: Uuid::from_u128(1),
		state: "running".into(),
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
	assert_eq!(
		change.result,
		json!({"termination_confirmed":stopped,"effects_may_have_occurred":effects,"error":"AUTHORITY_WITHDRAWN"})
	);
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
