use super::*;
use rstest::{fixture, rstest};
use serde_json::json;

#[fixture]
fn raw() -> RawRun {
	serde_json::from_value(json!({
        "id":Uuid::from_u128(1),"task_id":Uuid::from_u128(2),"workspace_id":Uuid::from_u128(3),
        "node":"brave-otter-k3m9q2xa","home_node":"aidash://home/brave-otter-k3m9q2xa","agent_id":"producer","agent_version":"1",
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
		json!({"retry":null,"lease_recovered":true,"decision_cycle":1})
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
fn each_manual_retry_starts_a_new_decision_cycle(mut raw: RawRun) {
	raw.pending["recovery"]["decision_cycle"] = json!(4);
	let result = raw.resumed_pending().unwrap().unwrap();
	assert_eq!(result["recovery"]["decision_cycle"], json!(5));
}
#[rstest]
fn manual_context_retry_clears_its_typed_reason_and_schedule(mut raw: RawRun) {
	let recovery = raw.pending["recovery"].as_object_mut().unwrap();
	recovery.remove("semantic_reason");
	recovery.insert("context_reason".into(), json!("output_truncated"));
	let result = raw.resumed_pending().unwrap().unwrap();
	assert_eq!(result["data"], raw.pending["data"]);
	assert_eq!(
		result["recovery"],
		json!({"retry":null,"lease_recovered":true,"decision_cycle":1})
	);
}
#[rstest]
fn a_manual_retry_of_a_semantic_pause_names_its_cause(raw: RawRun) {
	assert_eq!(
		raw.manual_semantic_retry().unwrap(),
		Some(crate::semantic::Failure::RetriesExhausted)
	);
}
#[rstest]
#[case::active(RunControl::Active)]
#[case::cancelled(RunControl::Cancelled)]
fn only_a_paused_run_opens_a_retry_cycle(mut raw: RawRun, #[case] control: RunControl) {
	raw.metadata.control = control;
	assert_eq!(raw.manual_semantic_retry().unwrap(), None);
}
#[rstest]
fn a_context_pause_does_not_open_a_retry_cycle(mut raw: RawRun) {
	let recovery = raw.pending["recovery"].as_object_mut().unwrap();
	recovery.remove("semantic_reason");
	recovery.insert("context_reason".into(), json!("output_truncated"));
	assert_eq!(raw.manual_semantic_retry().unwrap(), None);
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
		json!({"retry":null,"lease_recovered":true,"decision_cycle":1})
	);
	assert!(raw.decode().is_err());
}

#[rstest]
fn inspection_omits_source_text_while_storage_preserves_the_retry_cache(mut raw: RawRun) {
	let mut context = Context::default();
	context.source_observation = Some(
		crate::context::sources::SourceObservation::new(
			"0:0:0".into(),
			"sha256:fixture".into(),
			json!({"skill_context":"private-source-text"}),
		)
		.unwrap(),
	);

	let contribution = crate::context::source::SourceContribution {
		placement: None,
		mandatory: false,
		identity: crate::context::source::CandidateIdentity {
			source: crate::registry::bindings::QualifiedRef::builtin(
				"aidash://node",
				"private-source-identity",
			),
			item: "private-item".into(),
			revision: "r1".into(),
			range: None,
		},
		content: json!("private-contribution-text"),
		provenance: vec![],
		dependencies: vec![],
		completeness: crate::context::source::Completeness::Complete,
		detail: None,
		fetched_digest: crate::registry::rules::digest(&json!("private-contribution-text")),
		content_digest: crate::registry::rules::digest(&json!("private-contribution-text")),
	};
	context.source_deliveries.push(
		crate::context::source::accepted_delivery(
			&contribution,
			crate::registry::rules::digest(&json!("private-request")),
			uuid::Uuid::from_u128(9),
		)
		.unwrap(),
	);
	context.source_contributions.push(contribution);
	raw.context = serde_json::to_value(&context).unwrap();
	let restored: Context = serde_json::from_value(raw.context.clone()).unwrap();
	assert_eq!(
		restored.source_observation.as_ref().unwrap().content,
		json!({"skill_context":"private-source-text"})
	);
	let inspection = serde_json::to_value(raw.inspect()).unwrap();
	assert!(inspection["context"].get("source_observation").is_none());
	assert!(!inspection.to_string().contains("private-source-text"));
	assert_eq!(restored.source_contributions.len(), 1);
	assert_eq!(restored.source_deliveries.len(), 1);
	assert!(!inspection.to_string().contains("private-contribution-text"));
	assert!(!inspection.to_string().contains("private-source-identity"));
	for field in [
		"source_contributions",
		"source_deliveries",
		"source_selections",
	] {
		assert!(inspection["context"].get(field).is_none());
	}
	let schema = serde_json::to_value(schemars::schema_for!(RunInspection)).unwrap();
	assert!(!schema.to_string().contains("source_observation"));
	for field in [
		"source_contributions",
		"source_deliveries",
		"source_selections",
	] {
		assert!(!schema.to_string().contains(field));
	}
}
