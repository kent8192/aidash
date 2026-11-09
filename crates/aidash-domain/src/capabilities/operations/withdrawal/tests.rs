use super::*;
use rstest::{fixture, rstest};
#[fixture]
fn snapshot() -> Snapshot {
	Snapshot {
		operation_id: Uuid::from_u128(1),
		digest: "immutable-intent".into(),
		runner_instance: Some("original-runner".into()),
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
#[case::prepared("prepared", WriterStop::Confirmed, "withdrawn", false, "active")]
#[case::stopped("running", WriterStop::Confirmed, "withdrawn", true, "active")]
#[case::pending("submitted", WriterStop::Unconfirmed, "cancelling", true, "uncertain")]
fn dispatched_writers_keep_possible_effects_even_after_a_confirmed_stop(
	mut snapshot: Snapshot,
	#[case] state: &str,
	#[case] stop: WriterStop,
	#[case] status: &str,
	#[case] effects: bool,
	#[case] area: &str,
) {
	snapshot.state = state.into();
	let change = snapshot.change(stop);
	assert_eq!(change.state, status);
	assert_eq!(change.area_state, Some(area));
	let mut expected = json!({"termination_confirmed":stop != WriterStop::Unconfirmed,"effects_may_have_occurred":effects,"error":"AUTHORITY_WITHDRAWN"});
	if effects {
		expected["runner_acknowledged"] = json!(false);
	}
	assert_eq!(change.result, expected);
}
#[rstest]
fn a_withdrawal_marks_the_operation_even_before_the_writer_is_confirmed_stopped(
	mut snapshot: Snapshot,
) {
	assert!(!snapshot.marked_withdrawn());
	snapshot.result = snapshot.change(WriterStop::Unconfirmed).result;
	assert!(snapshot.marked_withdrawn());
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
	assert_eq!(snapshot.change(WriterStop::Confirmed).area_state, None);
	assert_eq!(snapshot.change(WriterStop::NeverRan).area_state, None);
}

#[rstest]
fn an_uncertain_terminal_writer_reopens_only_its_own_area_after_stop(mut snapshot: Snapshot) {
	snapshot.state = "uncertain".into();
	snapshot.area_state = "uncertain".into();
	assert_eq!(snapshot.confirm_terminal_stop(false).area_state, None);
	assert_eq!(
		snapshot.confirm_terminal_stop(true).area_state,
		Some("active")
	);
	snapshot.area_epoch += 1;
	assert_eq!(snapshot.confirm_terminal_stop(true).area_state, None);
}
/// A withdrawn Check keeps the Area fenced only while its writer may run: it
/// never exports `/work`, so a confirmed stop reopens the Area, including one
/// an earlier unconfirmed pass fenced.
#[rstest]
#[case::confirmed_running_area(WriterStop::Confirmed, "running", Some("active"))]
#[case::pending_stop(WriterStop::Unconfirmed, "running", Some("uncertain"))]
#[case::confirmed_after_a_fenced_pass(WriterStop::Confirmed, "uncertain", Some("active"))]
#[case::still_unconfirmed(WriterStop::Unconfirmed, "uncertain", None)]
fn a_withdrawn_check_reopens_the_area_once_its_writer_stopped(
	mut snapshot: Snapshot,
	#[case] stop: WriterStop,
	#[case] area: &str,
	#[case] expected: Option<&str>,
) {
	snapshot.kind = super::super::VERIFICATION.into();
	snapshot.state = "cancelling".into();
	snapshot.area_state = area.into();
	assert_eq!(snapshot.change(stop).area_state, expected);
}
#[rstest]
fn a_withdrawn_shell_reopens_after_a_confirmed_stop(mut snapshot: Snapshot) {
	snapshot.state = "cancelling".into();
	snapshot.area_state = "uncertain".into();
	assert_eq!(
		snapshot.change(WriterStop::Confirmed).area_state,
		Some("active")
	);
}
/// The Runner refused the send or stopped the command before it started, so
/// nothing changed, whichever pass of the withdrawal learned it.
#[rstest]
#[case::running_area("submitted", "running")]
#[case::fenced_by_an_earlier_pass("cancelling", "uncertain")]
fn a_command_that_never_ran_has_no_effects_and_reopens_its_area(
	mut snapshot: Snapshot,
	#[case] state: &str,
	#[case] area: &str,
) {
	snapshot.state = state.into();
	snapshot.area_state = area.into();
	let change = snapshot.change(WriterStop::NeverRan);
	assert_eq!(change.state, "withdrawn");
	assert_eq!(change.area_state, Some("active"));
	assert_eq!(
		change.result,
		json!({"termination_confirmed":true,"effects_may_have_occurred":false,"error":"AUTHORITY_WITHDRAWN","runner_acknowledged":false})
	);
}
#[rstest]
#[case::absent(json!({"status":"absent"}), WriterStop::Unconfirmed)]
#[case::still_stopping(json!({"termination_confirmed":false,"executed":false}), WriterStop::Unconfirmed)]
#[case::stopped_without_saying(json!({"termination_confirmed":true}), WriterStop::Confirmed)]
#[case::stopped_after_running(json!({"termination_confirmed":true,"executed":true}), WriterStop::Confirmed)]
#[case::lost_track(json!({"status":"uncertain","termination_confirmed":true,"executed":false}), WriterStop::Confirmed)]
#[case::never_ran(json!({"status":"cancelled","termination_confirmed":true,"executed":false}), WriterStop::NeverRan)]
fn only_an_explicit_reply_proves_a_stop_or_that_nothing_ran(
	#[case] reply: Value,
	#[case] stop: WriterStop,
) {
	assert_eq!(WriterStop::from_reply(&reply), stop);
}
#[rstest]
#[case::pending("submitted", true)]
#[case::pending_after_an_earlier_pass("cancelling", true)]
#[case::already_settled("withdrawn", false)]
fn only_an_active_pending_operation_asks_for_a_refusal(
	mut snapshot: Snapshot,
	#[case] state: &str,
	#[case] refused: bool,
) {
	snapshot.state = state.into();
	snapshot.result = json!({"dispatch_pending":true});
	assert_eq!(
		snapshot.undelivered_refusal(),
		refused.then(|| json!({"digest":"immutable-intent","instance":"original-runner"}))
	);
}

#[rstest]
#[case::confirmed("uncertain", json!({"termination_confirmed":true,"runner_acknowledged":true}), Some("runner"), true)]
#[case::ack_only("completed", json!({"runner_acknowledged":true}), Some("runner"), false)]
#[case::stop_only("completed", json!({"termination_confirmed":true}), Some("runner"), false)]
#[case::undispatched("withdrawn", json!({"effects_may_have_occurred":false}), None, true)]
#[case::uncertain_no_shortcut("uncertain", json!({"effects_may_have_occurred":false}), None, false)]
#[case::malformed("failed", json!({"termination_confirmed":"true","runner_acknowledged":true}), Some("runner"), false)]
fn deletion_requires_exact_physical_completion(
	#[case] state: &str,
	#[case] result: Value,
	#[case] instance: Option<&str>,
	#[case] expected: bool,
) {
	assert_eq!(safe_to_erase(state, &result, instance), expected);
}
