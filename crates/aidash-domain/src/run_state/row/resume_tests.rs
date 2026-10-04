use super::*;
use rstest::{fixture, rstest};
use serde_json::json;

#[fixture]
fn raw() -> RawRun {
	serde_json::from_value(json!({
        "id":Uuid::from_u128(1),"task_id":Uuid::from_u128(2),"workspace_id":Uuid::from_u128(3),
        "home_node":"aidash://home","agent_id":"producer","agent_version":"1",
        "phase":"THINKING","control":"PAUSED","step":3,"revision":7,
        "observed_input_seq":9,"ledger_worker_ready":true,"error":"semantic pause",
        "lease_owner":null,"lease_until":null,"updated_at":"2030-01-01T00:00:00Z",
        "context":Context::default(),
        "pending":{"state_version":1,"data":{"force_workspace_read_compaction":true,
            "deferred_workspace_read":null,"deferred_skill_read":null,"deferred_workspace_observation":null,
            "selected_media":[],"media_intake_through_seq":6,"deferred_run_message_reads":[Uuid::from_u128(4)]},
            "recovery":{"semantic_reason":"retries_exhausted","retry":{"count":3,"at":"2030-01-01T00:00:00Z"},"lease_recovered":true}}
    })).unwrap()
}
#[rstest]
fn manual_semantic_retry_clears_only_its_reason_and_schedule_and_preserves_the_continuation(
	raw: RawRun,
) {
	let result = raw.resumed_pending().unwrap().unwrap();
	assert_eq!(result["data"], raw.pending["data"]);
	assert_eq!(result["state_version"], json!(1));
	assert_eq!(
		result["recovery"],
		json!({"retry":null,"lease_recovered":true})
	);
	assert_eq!(
		raw.pending["recovery"]["semantic_reason"],
		json!("retries_exhausted")
	);
	assert_eq!(raw.revision, 7);
	assert_eq!(raw.control, RunControl::Paused);
}
#[rstest]
#[case::active(RunControl::Active)]
#[case::cancelled(RunControl::Cancelled)]
fn a_nonpaused_run_keeps_its_existing_semantic_schedule(
	mut raw: RawRun,
	#[case] control: RunControl,
) {
	raw.metadata.control = control;
	assert_eq!(raw.resumed_pending().unwrap(), None);
}
#[rstest]
fn a_pause_without_a_semantic_reason_does_not_clear_ordinary_recovery(mut raw: RawRun) {
	raw.pending["recovery"]
		.as_object_mut()
		.unwrap()
		.remove("semantic_reason");
	assert_eq!(raw.resumed_pending().unwrap(), None);
}
#[rstest]
#[case::unsupported_state("state")]
#[case::incomplete_pending("pending")]
#[case::malformed_context("context")]
fn executable_resumes_reject_corrupt_state_instead_of_inventing_defaults(
	mut raw: RawRun,
	#[case] field: &str,
) {
	match field {
		"state" => raw.pending["state_version"] = json!(2),
		"pending" => raw.pending = Value::Null,
		"context" => raw.context = Value::Null,
		_ => panic!("unknown field"),
	}
	assert!(raw.resumed_pending().is_err());
}
#[rstest]
fn a_failure_delivery_can_resume_without_decoding_a_corrupt_diagnostic_context(mut raw: RawRun) {
	raw.metadata.phase = RunPhase::Waiting;
	raw.context = Value::Null;
	raw.pending["data"] = json!({"reason":"failure_delivery","target":"CANCELLED",
        "wake_at":"2030-01-01T00:00:00Z","last_delivery_error":"peer unavailable"});
	let result = raw.resumed_pending().unwrap().unwrap();
	assert_eq!(result["data"], raw.pending["data"]);
	assert_eq!(
		result["recovery"],
		json!({"retry":null,"lease_recovered":true})
	);
	assert!(raw.decode().is_err());
}
