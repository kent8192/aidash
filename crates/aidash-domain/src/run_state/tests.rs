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
