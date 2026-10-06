use super::*;
use crate::{RawRun, context::Context};
use rstest::{fixture, rstest};
use serde_json::json;
use uuid::Uuid;
#[fixture]
fn run() -> Run {
	serde_json::from_value::<RawRun>(json!({
        "id":Uuid::from_u128(1),"task_id":Uuid::from_u128(2),"workspace_id":Uuid::from_u128(3),
        "home_node":"aidash://home","agent_id":"agent","agent_version":"1","phase":"READY","control":"ACTIVE",
        "step":4,"revision":7,"observed_input_seq":9,"ledger_worker_ready":true,"error":"saved diagnostic",
        "lease_owner":Uuid::from_u128(5),"lease_until":"2030-01-01T00:00:00Z","updated_at":"2030-01-01T00:00:00Z",
        "context":Context::default(),"pending":{"state_version":1,"data":{},"recovery":{"retry":{"count":3,"at":"2030-01-01T00:00:00Z"},"lease_recovered":true}}
    })).unwrap().decode().unwrap()
}
#[rstest]
#[case::retry("run.retrying", true, true)]
#[case::semantic_retry("run.semantic_retrying", true, true)]
#[case::failure_delivery("run.failure_pending", true, true)]
#[case::failed("run.failed", false, true)]
#[case::model_completed("model.completed", false, false)]
#[case::run_completed("run.completed", false, false)]
#[case::cancelled("run.cancelled", false, false)]
#[case::unknown("run.future", false, false)]
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
	assert_eq!(snapshot.pending, pending);
	assert_eq!(snapshot.error, keep_error.then_some("saved diagnostic"));
	assert_eq!(serde_json::to_value(&run).unwrap(), before);
	assert_eq!(snapshot.pending["recovery"]["lease_recovered"], true);
}
#[rstest]
fn absent_diagnostics_and_retries_remain_absent_for_retry_events(mut run: Run) {
	run.error = None;
	run.recovery.retry = None;
	let snapshot = run.worker_snapshot("run.retrying").unwrap();
	assert_eq!(snapshot.error, None);
	assert_eq!(snapshot.pending["recovery"]["retry"], Value::Null);
	assert_eq!(snapshot.pending, run.stored_pending().unwrap());
}
