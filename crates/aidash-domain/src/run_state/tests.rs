use super::*;
use serde_json::json;

// Handwritten fixtures are independent of the serializer/default constructors.
fn tool_data() -> Value {
	json!({
		"response":{"text":"answer","tool_calls":[{"id":"call-1","name":"echo","arguments":{"arbitrary":[1,null]}}],"input_tokens":1,"output_tokens":2,"usage_complete":true},
		"response_epoch":9,"cursor":0,"included_input_seq":3,"request_window":128000,"request_tokens":100,"finalizing":false,
		"media_inferred_seq_before_response":0,"observed_input_seq_before_response":0,"inferred_selected_media":[],"deferred_selected_media":[],"selected_media":[],"deferred_human_media":false,
		"media_inferred_through_seq":null,"media_intake_through_seq":0,"required_run_message_reads":[],"references_read_at_inference":true,"run_message_catchup":false,"run_message_summary_end_seq":0,"run_message_summary_limit":1024,
		"deferred_reads":thinking_data(),"workspace_read_plan":null,"skill_read_plan":null,"workspace_observation_plan":null,"home_approval":null,"approval":null
	})
}
fn thinking_data() -> Value {
	json!({"force_workspace_read_compaction":false,"deferred_workspace_read":null,"deferred_skill_read":null,"deferred_workspace_observation":null,"selected_media":[],"media_intake_through_seq":null,"deferred_run_message_reads":[]})
}
fn envelope(data: Value) -> Value {
	json!({"state_version":1,"data":data,"recovery":{"retry":null}})
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
		(RunPhase::Waiting,json!({"reason":"approval","approval":{"source":"capability","approval_id":id},"resume":tool_data()})),
		(RunPhase::Waiting,json!({"reason":"approval","approval":{"source":"home","request_id":id,"key":"run:9:0","call":{"id":"call-1","name":"echo","arguments":{}},"expires_at":date},"resume":tool_data()})),
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
		json!({"state_version":2,"data":{},"recovery":{"retry":null}}),
		json!({"state_version":1,"data":{},"recovery":{"retry":null},"future":true}),
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
#[case("durable_memory_budget")]
#[case("allowance")]
#[case("unavailable")]
#[case("retries_exhausted")]
#[case("pending")]
fn semantic_recovery_retains_its_reason_and_retry_deadline(#[case] reason: &str) {
	let input = json!({"state_version":1,"data":thinking_data(),"recovery":{
		"retry":{"count":2,"at":"2030-01-01T00:00:00Z"},
		"semantic_reason":reason
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
	assert!(StateRequirements::of(&state, &recovery).is_empty());
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
		json!({"state_version":1,"data":{},"recovery":{"retry":null},"future":true}),
	)
	.unwrap_err();
	let future_version = decode(
		RunPhase::Ready,
		json!({"state_version":2,"data":{},"recovery":{"retry":null}}),
	)
	.unwrap_err();

	// Assert: each is the typed mismatch, while real damage stays `Invalid`.
	for error in [
		undeclared,
		declared,
		future_variant,
		future_envelope,
		future_version,
	] {
		assert!(matches!(error, Error::UnsupportedState(_)), "{error:?}");
	}
	let mut damaged = tool_data();
	damaged.as_object_mut().unwrap().remove("response_epoch");
	assert!(matches!(
		decode(RunPhase::ToolCall, envelope(damaged)),
		Err(Error::Invalid(_))
	));
	assert!(matches!(
		decode(
			RunPhase::Ready,
			json!({"state_version":0,"data":{},"recovery":{"retry":null}}),
		),
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
	assert!(StateRequirements::of(&state, &recovery).is_empty());
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
	assert_eq!(
		StateRequirements::of(&state, &recovery).names(),
		["tool_batch_state"]
	);
	assert_eq!(saved, requiring(&["tool_batch_state"], data));
	let (plain, recovery) = decode(RunPhase::ToolCall, envelope(tool_data())).unwrap();
	assert!(StateRequirements::of(&plain, &recovery).is_empty());
}
#[rstest::rstest]
fn degraded_context_requires_the_degraded_context_capability() {
	// Arrange: a step ran with Degraded context (ADR 0059).
	let mut degraded = envelope(json!({}));
	degraded["recovery"]["degraded"] =
		json!({"count":1,"recent":[{"step":0,"cause":"vector_backend"}],"outage":true});
	let mut reserved = degraded.clone();
	reserved["requires"] = json!(["degraded_context"]);

	// Act
	let (state, recovery) = decode(RunPhase::Ready, reserved.clone()).unwrap();
	let short = decode(RunPhase::Ready, degraded).unwrap_err();

	// Assert: the requirement is derived from the recovery state and written on
	// save, and an envelope that omits it would let an older worker claim it.
	assert_eq!(
		StateRequirements::of(&state, &recovery).names(),
		["degraded_context"]
	);
	assert_eq!(encode(&state, &recovery).unwrap(), reserved);
	assert!(matches!(&short, Error::Invalid(message) if message.contains("does not cover")));
}
#[rstest::rstest]
fn a_requirement_that_falls_short_is_invalid() {
	// Arrange: a Tool Batch whose envelope does not declare `tool_batch_state`,
	// so a worker without that capability could lease it.
	let mut data = tool_data();
	data["batch_end"] = json!(1);

	// Act
	let error = decode(RunPhase::ToolCall, envelope(data)).unwrap_err();

	// Assert
	assert!(
		matches!(&error, Error::Invalid(message) if message.contains("does not cover")),
		"{error:?}"
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
		node: crate::nodes::NodeName::parse("brave-otter-k3m9q2xa").unwrap(),
		home_node: "aidash://home".into(),
		agent_id: "fixture".into(),
		agent_version: "1.0.0".into(),
		phase: RunPhase::ToolCall,
		control: crate::RunControl::Active,
		step: 0,
		revision: 0,
		observed_input_seq: 0,
		ledger_worker_ready: false,
		control_revision: 0,
		control_source: Default::default(),
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
	run.control = crate::RunControl::Cancelled;
	assert_eq!(run.lease_wait(&requires), None);
}

#[rstest::rstest]
fn advancing_the_cursor_leaves_the_approvals_of_earlier_calls_behind() {
	let mut state: ToolCallState = serde_json::from_value(tool_data()).unwrap();
	state.approval = Some(Uuid::from_u128(1));
	state.home_approval = Some(HomeApproval {
		key: "run:9:0".into(),
		call: state.response.tool_calls[0].clone(),
		approved: true,
	});

	state.advance(1);

	assert_eq!(state.cursor, 1);
	assert!(state.approval.is_none() && state.home_approval.is_none());
}

#[rstest::rstest]
fn only_an_approval_asked_of_another_node_has_a_deadline_and_a_human_request() {
	let id = Uuid::from_u128(7);
	let date: DateTime<Utc> = "2026-10-19T00:00:00Z".parse().unwrap();
	let resume: ToolCallState = serde_json::from_value(tool_data()).unwrap();
	let capability = WaitingState::Approval {
		approval: ApprovalRef::Capability { approval_id: id },
		resume: Box::new(resume.clone()),
	};
	let home = WaitingState::Approval {
		approval: ApprovalRef::Home {
			request_id: id,
			key: "run:9:0".into(),
			call: resume.response.tool_calls[0].clone(),
			expires_at: date,
		},
		resume: Box::new(resume),
	};

	assert_eq!(
		(capability.deadline(), capability.request_id()),
		(None, None)
	);
	assert_eq!((home.deadline(), home.request_id()), (Some(date), Some(id)));
}

#[rstest::rstest]
fn a_question_with_an_answer_deadline_wakes_the_run_then_and_any_other_waits_for_its_answer() {
	let id = Uuid::from_u128(7);
	let date: DateTime<Utc> = "2026-10-19T00:00:00Z".parse().unwrap();
	let human = |answer_by| WaitingState::Human {
		request_id: id,
		answer_by,
		resume: ResumeState::Ready(ReadyState {}),
	};

	assert_eq!(human(Some(date)).deadline(), Some(date));
	assert_eq!(human(None).deadline(), None);
	assert_eq!(human(Some(date)).request_id(), Some(id));
}

#[rstest::rstest]
fn the_answer_deadline_is_stored_only_when_there_is_one() {
	let id = "0b31d0f7-cd83-4f5a-adb8-0d23abb44ebf";
	let without = json!({"reason":"human","request_id":id,"resume":{"phase":"READY","data":{}}});
	let with = json!({"reason":"human","request_id":id,"answer_by":"2026-10-19T00:00:00Z","resume":{"phase":"READY","data":{}}});

	for data in [without, with] {
		let (state, recovery) = decode(RunPhase::Waiting, envelope(data.clone())).unwrap();
		let stored = encode(&state, &recovery).unwrap();
		assert_eq!(stored["data"], data);
	}
}

#[rstest::rstest]
fn the_retired_approval_waits_cannot_become_executable() {
	let id = "0b31d0f7-cd83-4f5a-adb8-0d23abb44ebf";
	for data in [
		json!({"reason":"core_approval","approval_id":id,"resume":thinking_data()}),
		json!({"reason":"external_approval","request_id":id,"key":"run:9:0","call":{"id":"call-1","name":"echo","arguments":{}},"expires_at":"2026-10-19T00:00:00Z","resume":tool_data()}),
	] {
		assert!(decode(RunPhase::Waiting, envelope(data)).is_err());
	}
}

#[test]
fn a_saved_mcp_checkpoint_requires_a_worker_that_understands_its_continuation() {
	let mut data = tool_data();
	data["mcp"] = json!({"key":"call","continuation_sends":1,"wire_pending":false,"input":{"request_state":"opaque","requests":{}},"responses":{},"active":null,"waited_millis":5000});
	let mut sent_data = data.clone();
	sent_data["mcp"]["sent"] = json!({"key":"call:mcp:round:1","input":{"call":{"id":"call-1","name":"echo","arguments":{}},"request_state":"opaque","input_responses":{}}});
	for (data, requirements) in [
		(data, vec!["mcp_call_state"]),
		(sent_data, vec!["mcp_call_state", "mcp_sent_round"]),
	] {
		let pending = requiring(&requirements, data.clone());
		let (state, recovery) = decode(RunPhase::ToolCall, pending.clone()).unwrap();
		assert_eq!(
			StateRequirements::of(&state, &recovery).names(),
			requirements
		);
		assert_eq!(encode(&state, &recovery).unwrap(), pending);
		assert!(decode(RunPhase::ToolCall, envelope(data)).is_err());
	}
}

#[test]
fn lease_recovery_counter_defaults_to_zero_and_only_persists_nonzero() {
	let recovery: RecoveryState = serde_json::from_value(json!({"retry": null})).unwrap();
	assert_eq!(recovery.lease_recoveries, 0);
	assert!(
		serde_json::to_value(&recovery)
			.unwrap()
			.get("lease_recoveries")
			.is_none()
	);
	let recovery = RecoveryState {
		lease_recoveries: 3,
		..recovery
	};
	let saved = serde_json::to_value(&recovery).unwrap();
	assert_eq!(saved["lease_recoveries"], 3);
	assert_eq!(
		serde_json::from_value::<RecoveryState>(saved)
			.unwrap()
			.lease_recoveries,
		3
	);
}

#[test]
fn legacy_lease_flags_are_read_and_saved_as_counters() {
	for (flag, count) in [(false, 0), (true, 1)] {
		let mut pending = envelope(json!({}));
		pending["recovery"]["lease_recovered"] = json!(flag);
		let (state, recovery) = decode(RunPhase::Ready, pending).unwrap();
		assert_eq!(recovery.lease_recoveries, count);
		let saved = encode(&state, &recovery).unwrap();
		assert!(saved["recovery"].get("lease_recovered").is_none());
	}
}
