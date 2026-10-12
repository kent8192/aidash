use super::*;
use serde_json::json;

// Handwritten fixtures are independent of the serializer/default constructors.
fn tool_data() -> Value {
	json!({
		"response":{"text":"answer","tool_calls":[{"id":"call-1","name":"echo","arguments":{"arbitrary":[1,null]}}],"input_tokens":1,"output_tokens":2,"usage_complete":true},
		"response_epoch":9,"cursor":0,"included_input_seq":3,"request_window":128000,"request_tokens":100,"finalizing":false,
		"media_inferred_seq_before_response":0,"observed_input_seq_before_response":0,"inferred_selected_media":[],"deferred_selected_media":[],"selected_media":[],"deferred_human_media":false,
		"media_inferred_through_seq":null,"media_intake_through_seq":0,"required_run_message_reads":[],"references_read_at_inference":true,"run_message_catchup":false,"run_message_summary_end_seq":0,"run_message_summary_limit":1024,
		"deferred_reads":thinking_data(),"workspace_read_plan":null,"skill_read_plan":null,"workspace_observation_plan":null,"workbench_approval_result":null
	})
}
fn thinking_data() -> Value {
	json!({"force_workspace_read_compaction":false,"deferred_workspace_read":null,"deferred_skill_read":null,"deferred_workspace_observation":null,"selected_media":[],"media_intake_through_seq":null,"deferred_run_message_reads":[]})
}
fn envelope(data: Value) -> Value {
	json!({"state_version":1,"data":data,"recovery":{"retry":null,"lease_recovered":false}})
}
#[rstest::rstest]
fn current_storage_covers_every_phase_and_wait_continuation() {
	let id = "0b31d0f7-cd83-4f5a-adb8-0d23abb44ebf";
	let date = "2030-01-01T00:00:00Z";
	let resumes = [
		json!({"phase":"READY","data":{}}),
		json!({"phase":"THINKING","data":thinking_data()}),
		json!({"phase":"TOOL_CALL","data":tool_data()}),
	];
	let mut cases = vec![
		(RunPhase::Ready, json!({})),
		(RunPhase::Thinking, thinking_data()),
		(RunPhase::ToolCall, tool_data()),
		(RunPhase::Completed, json!({})),
		(RunPhase::Failed, json!({})),
		(RunPhase::Cancelled, json!({})),
	];
	for resume in resumes {
		cases.push((
			RunPhase::Waiting,
			json!({"reason":"human","request_id":id,"resume":resume}),
		));
		cases.push((
			RunPhase::Waiting,
			json!({"reason":"timer","wake_at":date,"resume":resume}),
		));
	}
	cases.extend([
		(RunPhase::Waiting,json!({"reason":"dependencies","wake_at":date,"resume":{}})),
		(RunPhase::Waiting,json!({"reason":"children","wake_at":date,"resume":thinking_data()})),
		(RunPhase::Waiting,json!({"reason":"core_approval","approval_id":id,"resume":thinking_data()})),
		(RunPhase::Waiting,json!({"reason":"external_approval","request_id":id,"key":"run:9:0","call":{"id":"call-1","name":"echo","arguments":{}},"expires_at":date,"resume":tool_data()})),
		(RunPhase::Waiting,json!({"reason":"reconciliation","request_id":id,"key":"run:9:0","resume":tool_data()})),
		(RunPhase::Waiting,json!({"reason":"failure_delivery","target":"FAILED","wake_at":date,"last_delivery_error":null}))
	]);
	for (phase, data) in cases {
		let input = envelope(data);
		let (state, recovery) = decode(phase, input.clone()).unwrap();
		assert_eq!(state.phase(), phase);
		assert_eq!(encode(&state, &recovery).unwrap(), input);
	}
}
#[rstest::rstest]
fn unsupported_or_incomplete_machine_payloads_cannot_become_executable() {
	for input in [
		json!({}),
		json!([]),
		json!({"data":{}}),
		json!({"state_version":2,"data":{},"recovery":{"retry":null,"lease_recovered":false}}),
		json!({"state_version":1,"data":{},"recovery":{"retry":null,"lease_recovered":false},"future":true}),
	] {
		assert!(decode(RunPhase::Ready, input).is_err());
	}
	for data in [
		json!({"reason":"future_wait"}),
		json!({"reason":"timer","wake_at":"tomorrow","resume":{"phase":"READY","data":{}}}),
		json!({"reason":"human","request_id":"invalid","resume":{"phase":"READY","data":{}}}),
		json!({"reason":"human","request_id":"0b31d0f7-cd83-4f5a-adb8-0d23abb44ebf","resume":{"phase":"COMPLETED","data":{}}}),
	] {
		assert!(decode(RunPhase::Waiting, envelope(data)).is_err());
	}
	for change in [
		"missing_epoch",
		"invalid_cursor",
		"unknown_response",
		"incomplete_tool",
	] {
		let mut data = tool_data();
		match change {
			"missing_epoch" => {
				data.as_object_mut().unwrap().remove("response_epoch");
			}
			"invalid_cursor" => data["cursor"] = json!(2),
			"unknown_response" => data["response"]["future"] = json!(true),
			_ => {
				data["response"]["tool_calls"][0]
					.as_object_mut()
					.unwrap()
					.remove("id");
			}
		}
		assert!(
			decode(RunPhase::ToolCall, envelope(data)).is_err(),
			"{change}"
		);
	}
}

#[rstest::rstest]
fn required_content_distinguishes_missing_fields_from_explicit_null() {
	let call = json!({"id":"call-1","name":"echo","arguments":null});
	assert!(serde_json::from_value::<ToolCall>(call.clone()).is_ok());
	assert!(serde_json::from_value::<ToolCall>(json!({"id":"call-1","name":"echo"})).is_err());
	for (mut event, field) in [
		(json!({"kind":"tool","call":call,"result":null}), "result"),
		(
			json!({"kind":"human","request":"Question","request_kind":"INFORMATION_REQUEST","response":null}),
			"response",
		),
	] {
		assert!(serde_json::from_value::<crate::context::ContextEvent>(event.clone()).is_ok());
		event.as_object_mut().unwrap().remove(field);
		assert!(serde_json::from_value::<crate::context::ContextEvent>(event).is_err());
	}
	let mut plan = json!({"step":9,"cursor":0,"call":call,"result":null});
	assert!(serde_json::from_value::<ReadPlan>(plan.clone()).is_ok());
	plan.as_object_mut().unwrap().remove("result");
	assert!(serde_json::from_value::<ReadPlan>(plan).is_err());
}

#[rstest::rstest]
#[case("configuration")]
#[case("authority")]
#[case("invalidated")]
#[case("provider_contract")]
#[case("context_budget")]
#[case("allowance")]
#[case("unavailable")]
#[case("retries_exhausted")]
#[case("pending")]
fn semantic_recovery_retains_its_reason_and_retry_deadline(#[case] reason: &str) {
	let input = json!({"state_version":1,"data":thinking_data(),"recovery":{
		"retry":{"count":2,"at":"2030-01-01T00:00:00Z"},
		"lease_recovered":false,"semantic_reason":reason
	}});
	let (state, recovery) = decode(RunPhase::Thinking, input.clone()).unwrap();
	assert_eq!(recovery.retry.as_ref().unwrap().count, 2);
	assert_eq!(encode(&state, &recovery).unwrap(), input);
	let mut unknown = input;
	unknown["recovery"]["semantic_reason"] = json!("future_failure");
	assert!(decode(RunPhase::Thinking, unknown).is_err());
}

fn requiring(names: &[&str], data: Value) -> Value {
	let mut input = envelope(data);
	input["requires"] = json!(names);
	input
}
#[rstest::rstest]
fn a_state_without_requirements_keeps_the_legacy_envelope_bytes() {
	let (state, recovery) = decode(RunPhase::ToolCall, envelope(tool_data())).unwrap();

	let stored = encode(&state, &recovery).unwrap();

	assert!(stored.get("requires").is_none());
	assert_eq!(stored, envelope(tool_data()));
	assert!(StateRequirements::of(&state).is_empty());
}
#[rstest::rstest]
fn fields_a_worker_does_not_know_are_a_capability_mismatch_not_a_corrupt_run() {
	// Arrange: a newer build persisted a field this build's `deny_unknown_fields` rejects.
	let mut data = tool_data();
	data["future_field"] = json!(2);

	// Act
	let undeclared = decode(RunPhase::ToolCall, envelope(data.clone())).unwrap_err();
	let declared = decode(RunPhase::ToolCall, requiring(&["future_capability"], data)).unwrap_err();
	let future_variant =
		decode(RunPhase::Waiting, envelope(json!({"reason":"future_wait"}))).unwrap_err();
	let future_envelope = decode(
		RunPhase::Ready,
		json!({"state_version":1,"data":{},"recovery":{"retry":null,"lease_recovered":false},"future":true}),
	)
	.unwrap_err();

	// Assert: each is the typed mismatch, while real damage stays `Invalid`.
	for error in [undeclared, declared, future_variant, future_envelope] {
		assert!(matches!(error, Error::UnsupportedState(_)), "{error:?}");
	}
	let mut damaged = tool_data();
	damaged.as_object_mut().unwrap().remove("response_epoch");
	assert!(matches!(
		decode(RunPhase::ToolCall, envelope(damaged)),
		Err(Error::Invalid(_))
	));
}
#[rstest::rstest]
fn a_requirement_no_build_declares_is_a_capability_mismatch() {
	let error = decode(
		RunPhase::ToolCall,
		requiring(&["future_capability"], tool_data()),
	)
	.unwrap_err();

	assert!(matches!(&error, Error::UnsupportedState(m) if m.contains("future_capability")));
}
#[rstest::rstest]
fn a_stored_requirement_may_exceed_what_the_state_derives() {
	// Arrange: a newer build reserved a state for capable workers only.
	let reserved = requiring(&["tool_batch_state"], tool_data());

	// Act
	let (state, recovery) = decode(RunPhase::ToolCall, reserved).unwrap();

	// Assert: this build derives nothing, so re-saving drops the reservation.
	assert!(StateRequirements::of(&state).is_empty());
	assert_eq!(encode(&state, &recovery).unwrap(), envelope(tool_data()));
	assert!(StateRequirements::default().covered_by(&StateRequirements::default()));
	assert!(
		!StateRequirements::from_pending(&requiring(&["tool_batch_state"], json!({})))
			.unwrap()
			.covered_by(&StateRequirements::default())
	);
}
#[rstest::rstest]
fn a_tool_batch_end_requires_the_tool_batch_capability() {
	// Arrange: a Tool Batch admitted calls up to `batch_end` (#119).
	let mut data = tool_data();
	data["batch_end"] = json!(1);

	// Act
	let (state, recovery) = decode(
		RunPhase::ToolCall,
		requiring(&["tool_batch_state"], data.clone()),
	)
	.unwrap();
	let saved = encode(&state, &recovery).unwrap();

	// Assert: the requirement is derived from the state and written on save.
	assert_eq!(StateRequirements::of(&state).names(), ["tool_batch_state"]);
	assert_eq!(saved, requiring(&["tool_batch_state"], data));
	let (plain, _) = decode(RunPhase::ToolCall, envelope(tool_data())).unwrap();
	assert!(StateRequirements::of(&plain).is_empty());
}
#[rstest::rstest]
fn a_requirement_that_falls_short_is_read_and_rewritten() {
	// Arrange: a Tool Batch persisted by a build after #119 but before this gate,
	// so its envelope carries no requirement.
	let mut data = tool_data();
	data["batch_end"] = json!(1);

	// Act
	let (state, recovery) = decode(RunPhase::ToolCall, envelope(data.clone())).unwrap();

	// Assert: it is not treated as damage, and the next save adds the requirement.
	assert_eq!(
		encode(&state, &recovery).unwrap(),
		requiring(&["tool_batch_state"], data)
	);
}
#[rstest::rstest]
fn requirements_are_read_without_decoding_the_state() {
	let pending = requiring(&["tool_batch_state", "alpha", "alpha"], json!({"future":1}));

	let requires = StateRequirements::from_pending(&pending).unwrap();

	assert_eq!(requires.names(), ["alpha", "tool_batch_state"]);
	assert_eq!(requires.unmet(&[]), ["alpha", "tool_batch_state"]);
	assert_eq!(requires.unmet(PersistedCapability::supported()), ["alpha"]);
	assert!(
		StateRequirements::from_pending(&envelope(json!({})))
			.unwrap()
			.is_empty()
	);
	assert!(StateRequirements::from_pending(&json!({"requires":"tool_batch_state"})).is_err());
}
#[rstest::rstest]
fn a_runnable_run_that_needs_capabilities_is_waiting_for_a_capable_worker() {
	let requires =
		StateRequirements::from_pending(&requiring(&["tool_batch_state"], json!({}))).unwrap();
	let mut run = RunMetadata {
		id: Uuid::nil(),
		task_id: Uuid::nil(),
		workspace_id: Uuid::nil(),
		home_node: "aidash://home".into(),
		agent_id: "fixture".into(),
		agent_version: "1.0.0".into(),
		phase: RunPhase::ToolCall,
		control: crate::RunControl::Active,
		step: 0,
		revision: 0,
		observed_input_seq: 0,
		ledger_worker_ready: false,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: Utc::now(),
	};

	assert_eq!(
		run.lease_wait(&requires),
		Some(LeaseWait::CapableWorker {
			capabilities: vec!["tool_batch_state".into()]
		})
	);
	assert_eq!(run.lease_wait(&StateRequirements::default()), None);
	run.lease_until = Some(Utc::now() + chrono::Duration::seconds(30));
	assert_eq!(run.lease_wait(&requires), None);
	run.lease_until = None;
	run.control = crate::RunControl::Paused;
	assert_eq!(run.lease_wait(&requires), None);
}
