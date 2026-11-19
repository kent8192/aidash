use super::*;
use crate::{RawRun, context::Context};
use rstest::{fixture, rstest};
use serde_json::json;
use uuid::Uuid;

#[rstest]
fn outage_tracking_survives_wake_and_is_cleared_after_progress(mut run: Run) {
	let start = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z")
		.unwrap()
		.to_utc();
	run.recovery.outage = Some(crate::run_state::OutageTracking {
		service: "Decider".into(),
		started_at: start,
	});
	run.recovery.decider_retry = 6;
	let resumed = run.worker_snapshot("execution.outage_resumed").unwrap();
	assert_eq!(resumed.pending["recovery"]["outage"]["service"], "Decider");
	assert_eq!(resumed.pending["recovery"]["decider_retry"], 6);
	let completed = run.worker_snapshot("model.completed").unwrap();
	assert!(completed.pending["recovery"].get("outage").is_none());
	assert!(completed.pending["recovery"].get("decider_retry").is_none());
}
#[fixture]
fn run() -> Run {
	serde_json::from_value::<RawRun>(json!({
        "id":Uuid::from_u128(1),"task_id":Uuid::from_u128(2),"thread_id":Uuid::from_u128(3),
        "node":"brave-otter-k3m9q2xa","home_node":"aidash://home/brave-otter-k3m9q2xa","agent_id":"agent","agent_version":"1","phase":"READY","control":"ACTIVE",
        "step":4,"revision":7,"observed_input_seq":9,"ledger_worker_ready":true,"error":"saved diagnostic",
        "lease_owner":Uuid::from_u128(5),"lease_until":"2030-01-01T00:00:00Z","updated_at":"2030-01-01T00:00:00Z",
        "context":Context::default(),"pending":{"state_version":1,"data":{},"recovery":{"retry":{"count":3,"at":"2030-01-01T00:00:00Z"},"lease_recoveries":1}}
    })).unwrap().decode().unwrap()
}
#[rstest]
#[case::retry("execution.retrying", true, true)]
#[case::semantic_retry("execution.semantic_retrying", true, true)]
#[case::decider_retry("execution.decider_retrying", true, true)]
#[case::node_retry("execution.node_retrying", true, true)]
#[case::failure_delivery("execution.failure_pending", true, true)]
#[case::failed("execution.failed", false, true)]
#[case::model_completed("model.completed", false, false)]
#[case::run_completed("execution.completed", false, false)]
#[case::cancelled("execution.cancelled", false, false)]
#[case::unknown("execution.future", false, false)]
fn worker_save_keeps_only_the_event_specific_retry_and_error_disposition(
	run: Run,
	#[case] event: &str,
	#[case] keep_retry: bool,
	#[case] keep_error: bool,
) {
	let before = serde_json::to_value(&run).unwrap();
	let snapshot = run.worker_snapshot(event).unwrap();
	let mut pending = run.stored_pending().unwrap();
	if !keep_retry {
		pending["recovery"]["retry"] = Value::Null;
	}
	// None of these saves retains the lease, so each one ends the step.
	pending["recovery"]
		.as_object_mut()
		.unwrap()
		.remove("lease_recoveries");
	assert_eq!(snapshot.pending, pending);
	assert_eq!(snapshot.error, keep_error.then_some("saved diagnostic"));
	assert_eq!(serde_json::to_value(&run).unwrap(), before);
	assert_eq!(
		snapshot.pending["recovery"]["lease_recoveries"],
		Value::Null
	);
}
#[rstest]
#[case::sources("execution.sources_observed")]
#[case::degraded("execution.semantic_degraded")]
#[case::estimate("execution.estimate_pinned")]
#[case::mcp("execution.mcp_checkpoint")]
#[case::summary_revoked("context.summary_revoked")]
#[case::early_abandonment("execution.unassigned_tasks_abandoned")]
fn lease_retaining_checkpoints_keep_the_lease_recovery_count(run: Run, #[case] event: &str) {
	assert!(retains_lease(event));
	let snapshot = run.worker_snapshot(event).unwrap();
	assert_eq!(snapshot.pending["recovery"]["lease_recoveries"], 1);
}
#[rstest]
fn only_a_scheduled_decider_retry_carries_its_count_to_the_next_lease(mut run: Run) {
	run.recovery.decider_retry = 2;
	let scheduled = run.worker_snapshot("execution.decider_retrying").unwrap();
	assert_eq!(scheduled.pending["recovery"]["decider_retry"], 2);
	// Any other save, a generic retry included, starts the count again.
	for event in [
		"execution.retrying",
		"execution.semantic_retrying",
		"model.completed",
	] {
		let snapshot = run.worker_snapshot(event).unwrap();
		assert_eq!(
			snapshot.pending["recovery"]["decider_retry"],
			Value::Null,
			"{event}"
		);
	}
}
#[rstest]
fn only_a_scheduled_node_retry_carries_its_count_to_the_next_lease(mut run: Run) {
	run.recovery.node_retry = 3;
	let scheduled = run.worker_snapshot("execution.node_retrying").unwrap();
	assert_eq!(scheduled.pending["recovery"]["node_retry"], 3);
	for event in [
		"execution.retrying",
		"execution.decider_retrying",
		"execution.node_unreachable",
		"model.completed",
	] {
		let snapshot = run.worker_snapshot(event).unwrap();
		assert_eq!(
			snapshot.pending["recovery"]["node_retry"],
			Value::Null,
			"{event}"
		);
	}
}
#[rstest]
fn absent_diagnostics_and_retries_remain_absent_for_retry_events(mut run: Run) {
	run.error = None;
	run.recovery.retry = None;
	run.recovery.lease_recoveries = 0;
	let snapshot = run.worker_snapshot("execution.retrying").unwrap();
	assert_eq!(snapshot.error, None);
	assert_eq!(snapshot.pending["recovery"]["retry"], Value::Null);
	assert_eq!(snapshot.pending, run.stored_pending().unwrap());
}
