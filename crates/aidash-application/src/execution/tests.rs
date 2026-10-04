use crate::Error;
use aidash_domain::context::ContextEvent;
use serde_json::json;
use uuid::Uuid;
fn typed_event(value: serde_json::Value) -> ContextEvent {
	serde_json::from_value(value).unwrap()
}

fn pending(value: serde_json::Value) -> aidash_domain::ToolCallState {
	let mut complete = serde_json::to_value(aidash_domain::ToolCallState::default()).unwrap();
	for (key, value) in value.as_object().unwrap() {
		if key == "response" {
			let mut response = json!(aidash_domain::provider::ModelResponse::default());
			for (name, value) in value.as_object().unwrap() {
				response[name] = value.clone();
			}
			complete[key] = response;
		} else {
			complete[key] = value.clone();
		}
	}
	serde_json::from_value(complete).unwrap()
}

fn media_model() -> aidash_domain::model::ModelConfig {
	serde_json::from_value(json!({
		"provider":"openrouter", "model_id":"fixture", "endpoint":"https://example.com",
		"credential_env":null, "context_window":128000, "max_output_tokens":4096,
		"modalities":["text","image","audio"], "cost":{},
		"media_routes":[
			{"tag":"fixture/png", "formats":["image/png", "wav"],
			"source":"test", "verified_at":chrono::Utc::now() - chrono::Duration::hours(1),
			"expires_at":chrono::Utc::now() + chrono::Duration::hours(1)},
			{"tag":"fixture/jpeg", "formats":["image/jpeg"],
			"source":"test", "verified_at":chrono::Utc::now() - chrono::Duration::hours(1),
			"expires_at":chrono::Utc::now() + chrono::Duration::hours(1)}
		]
	}))
	.unwrap()
}

#[rstest::rstest]

fn stale_response_requeues_media_and_restores_its_inference_marker() {
	let first = json!({"file_id":Uuid::new_v4(),"expected_digest":"one"});
	let second = json!({"file_id":Uuid::new_v4(),"expected_digest":"two"});
	let mut context = aidash_domain::context::Context {
		media_inferred_seq: 5,
		..Default::default()
	};
	let mut observed_input_seq = 5;
	let required = Uuid::new_v4();
	let pending = pending(json!({
		"media_inferred_seq_before_response":3,
		"observed_input_seq_before_response":3,
		"media_intake_through_seq":2,
		"required_run_message_reads":[required],
		"inferred_selected_media":[first],
		"selected_media":[first,second]
	}));
	let next = super::stale_media_pending(&mut context, &pending, &mut observed_input_seq);
	assert_eq!(context.media_inferred_seq, 3);
	assert_eq!(observed_input_seq, 3);
	assert_eq!(json!(next.selected_media), json!([first, second]));
	assert_eq!(next.media_intake_through_seq, Some(2));
	assert_eq!(json!(next.deferred_run_message_reads), json!([required]));
}

#[test]

fn durable_media_observations_keep_recent_text_within_the_request_budget() {
	let mut context = aidash_domain::context::Context {
		history: vec![aidash_domain::context::ContextEvent::tool(
			aidash_domain::provider::ToolCall {
				id: "keep".into(),
				name: "read".into(),
				arguments: json!({}),
			},
			json!("keep"),
		)],
		..Default::default()
	};
	let budget = super::media_observation_budget(&pending(json!({"request_window":8192})));
	assert_eq!(budget, 512);
	for seq in 1..=20 {
		super::record_media_observation(&mut context, &"あ\\\"".repeat(1000), Some(seq), budget);
	}
	let observations: Vec<_> = context
		.history
		.iter()
		.filter(|event| matches!(event, ContextEvent::ModelMediaObservation { .. }))
		.collect();
	assert!(observations.len() < 20);
	assert_eq!(json!(observations.last().unwrap())["through_seq"], 20);
	assert_eq!(json!(observations.last().unwrap())["truncated"], true);
	assert!(
		observations
			.iter()
			.map(|event| event.to_string().len())
			.sum::<usize>()
			<= budget
	);
	assert_eq!(json!(context.history[0])["result"], "keep");
}

#[rstest::rstest]

fn human_media_takes_the_first_inference_when_selected_files_exceed_the_combined_cap() {
	let image = || aidash_domain::provider::ContentPart::Image {
		media_type: "image/png".into(),
		bytes: vec![1],
	};
	let (parts, deferred) = super::choose_inference_media(
		(0..8).map(|_| image()).collect(),
		vec![image()],
		128_000,
		true,
		&media_model(),
	);
	assert!(deferred);
	assert_eq!(parts.len(), 1);
}

#[rstest::rstest]

fn human_media_takes_the_first_inference_when_selected_audio_exceeds_headroom() {
	let audio = || aidash_domain::provider::ContentPart::Audio {
		format: "wav".into(),
		bytes: vec![0; 1024 * 1024],
	};
	let (parts, deferred) =
		super::choose_inference_media(vec![audio()], vec![audio()], 128_000, true, &media_model());
	assert!(deferred);
	assert_eq!(parts.len(), 1);
}

#[rstest::rstest]

fn text_only_batch_defers_selected_media_when_its_headroom_is_used() {
	let selected = aidash_domain::provider::ContentPart::Audio {
		format: "wav".into(),
		bytes: vec![0; 64 * 1024],
	};
	let (parts, deferred) =
		super::choose_inference_media(vec![selected], Vec::new(), 2_048, true, &media_model());
	assert!(deferred);
	assert!(parts.is_empty());
}

#[rstest::rstest]

fn selected_media_waits_when_human_media_needs_another_route() {
	let selected = aidash_domain::provider::ContentPart::Image {
		media_type: "image/jpeg".into(),
		bytes: vec![1],
	};
	let human = aidash_domain::provider::ContentPart::Image {
		media_type: "image/png".into(),
		bytes: vec![2],
	};
	let (parts, deferred) =
		super::choose_inference_media(vec![selected], vec![human], 128_000, true, &media_model());
	assert!(deferred);
	assert!(
		matches!(parts.as_slice(), [aidash_domain::provider::ContentPart::Image {media_type, ..}] if media_type == "image/png")
	);
}

#[rstest::rstest]

fn selected_files_must_share_one_current_route() {
	let png = aidash_domain::provider::ContentPart::Image {
		media_type: "image/png".into(),
		bytes: b"\x89PNG\r\n\x1a\nfirst".to_vec(),
	};
	let jpeg = aidash_domain::provider::ContentPart::Image {
		media_type: "image/jpeg".into(),
		bytes: b"\xff\xd8\xffsecond".to_vec(),
	};
	let model = media_model();
	assert!(super::check_model_media_headroom(128_000, vec![png.clone()], &model).is_ok());
	assert!(super::check_model_media_headroom(128_000, vec![jpeg.clone()], &model).is_ok());
	assert!(matches!(
		super::check_model_media_headroom(128_000, vec![png, jpeg], &model),
		Err(Error::Invalid(message)) if message.contains("no current media route")
	));
}

#[rstest::rstest]

fn encoded_message_text_consumes_media_headroom() {
	let text = "quoted \"text\" and newline\n".repeat(100);
	let reservation = super::encoded_run_message_reservation(&[json!({
		"seq":1,"sender":"human","content":text
	})]);
	assert!(reservation > text.len());
	let image = aidash_domain::provider::ContentPart::Image {
		media_type: "image/png".into(),
		bytes: vec![1],
	};
	let headroom = 4_096 + reservation / 2;
	assert!(super::media_request_headroom(headroom, std::slice::from_ref(&image)).is_ok());
	assert!(super::media_request_headroom(headroom - reservation, &[image]).is_err());
}

#[rstest::rstest]

fn selected_media_defers_workspace_mutations_until_the_next_inference() {
	for name in ["file_read", "file_search", "workspace_read", "skill_read"] {
		assert!(super::read_only_after_model_media_selection(name));
	}
	for name in [
		"skill_load",
		"apply_patch",
		"shell",
		"code_interpreter",
		"plugin_0",
	] {
		assert!(!super::read_only_after_model_media_selection(name));
	}
}

#[rstest::rstest]

fn deferred_tool_transitions_keep_selected_media() {
	let first = json!({"file_id":Uuid::new_v4(),"expected_digest":"first"});
	let second = json!({"file_id":Uuid::new_v4(),"expected_digest":"second"});
	let pending = pending(json!({
		"deferred_selected_media":[first],
		"selected_media":[second]
	}));
	assert_eq!(
		json!(super::pending_selected_media(&pending)),
		json!([first, second])
	);
}

#[rstest::rstest]

fn selected_audio_must_fit_the_model_context_before_tool_completion() {
	let mut bytes = vec![0_u8; 8 * 1024 * 1024];
	bytes[..12].copy_from_slice(b"RIFF\0\0\0\0WAVE");
	let part = aidash_domain::provider::ContentPart::from_media("audio/wav", bytes).unwrap();
	assert!(matches!(
		super::check_model_media_headroom(128_000, vec![part], &media_model()),
		Err(Error::Domain(aidash_domain::Error::Invalid(message))) if message.contains("context window")
	));
}

#[rstest::rstest]

fn small_context_windows_keep_their_available_budget() {
	assert!(super::request_context_window(2048, 1500) >= 1500);
	assert!(super::request_context_window(4096, 3000) >= 3000);
	assert!(super::request_context_window(32_000, 4000) < 32_000);
}

#[rstest::rstest]

fn discarded_response_keeps_a_fresh_effect_namespace_at_the_step_limit() {
	let last_step = 63;
	let old_response = super::response_epoch(20, last_step);
	let revision_after_discard = 21;
	let corrected_response = super::response_epoch(revision_after_discard, last_step);

	assert!(corrected_response > old_response);
}

#[rstest::rstest]

fn uninformed_responses_may_only_read_required_unread_messages() {
	let id = Uuid::new_v4();
	let unrelated_id = Uuid::new_v4();
	let required = [id];
	let context = aidash_domain::context::Context::default();
	let read_required = aidash_domain::provider::ToolCall {
		id: "required-read".into(),
		name: "workspace_read".into(),
		arguments: json!({"kind":"message","id":id}),
	};
	assert!(super::is_required_message_read(
		&read_required,
		&required,
		&context
	));
	let unrelated_read = aidash_domain::provider::ToolCall {
		arguments: json!({"kind":"message","id":unrelated_id}),
		..read_required.clone()
	};
	assert!(!super::is_required_message_read(
		&unrelated_read,
		&required,
		&context
	));
	let mutation = aidash_domain::provider::ToolCall {
		name: "workspace_message".into(),
		arguments: json!({"content":"change the workspace"}),
		..read_required.clone()
	};
	assert!(!super::is_required_message_read(
		&mutation, &required, &context
	));
	let mut read_context = context;
	read_context.message_read_coverage.insert(
		id,
		aidash_domain::context::MessageReadCoverage {
			total_chars: 10,
			ranges: vec![[0, 4]],
		},
	);
	assert!(!super::is_required_message_read(
		&read_required,
		&required,
		&read_context
	));
	let next_chunk = aidash_domain::provider::ToolCall {
		arguments: json!({"kind":"message","id":id,"offset":4}),
		..read_required.clone()
	};
	assert!(super::is_required_message_read(
		&next_chunk,
		&required,
		&read_context
	));
	let redundant_chunk = aidash_domain::provider::ToolCall {
		arguments: json!({"kind":"message","id":id,"offset":0}),
		..read_required
	};
	assert!(!super::is_required_message_read(
		&redundant_chunk,
		&required,
		&read_context
	));
}

#[rstest::rstest]

fn tight_windows_leave_room_for_the_pinned_workspace_context() {
	for slack in [2048, 4096, 8192] {
		let minimum_request = 20_000;
		let request_window =
			super::request_context_window(minimum_request + slack, minimum_request);
		let remaining = request_window.saturating_sub(minimum_request);
		assert!(
			remaining >= aidash_domain::context::MIN_CONTEXT_RESERVE,
			"slack {slack} consumed the admission reserve"
		);

		let snapshot_budget = remaining / 4;
		let mut pinned = serde_json::json!({
			"identity":{"node_id":"node-1","agent_id":"agent-1","agent_version":"1"},
			"task":{"id":"00000000-0000-0000-0000-000000000001","title":"task"},
			"workspace":{"workspace":{"id":"00000000-0000-0000-0000-000000000002","title":"workspace"}}
		});
		aidash_domain::context::bound_snapshot(&mut pinned, snapshot_budget).unwrap();
		assert!(aidash_domain::context::estimated_tokens(&pinned.to_string()) <= snapshot_budget);
		assert_eq!(pinned["identity"]["agent_id"], "agent-1");
		assert_eq!(pinned["task"]["id"], "00000000-0000-0000-0000-000000000001");
		assert_eq!(
			pinned["workspace"]["workspace"]["id"],
			"00000000-0000-0000-0000-000000000002"
		);
	}
}

#[rstest::rstest]

fn skill_read_fits_utf8_chunks_to_the_remaining_request_budget() {
	let call = aidash_domain::provider::ToolCall {
		id: "skill-1".into(),
		name: "skill_read".into(),
		arguments: serde_json::json!({"skill":{"id":"research","version":"1.0.0"},"path":"references/guide.md","offset":0,"max_chars":16000}),
	};
	let context = aidash_domain::context::Context::default();
	let output = serde_json::json!({"path":"references/guide.md","text":"界".repeat(5000),"encoding":"utf8","offset":0,"total_chars":15000,"next_offset":null});
	let full_event = typed_event(
		serde_json::json!({"kind":"tool","call":call,"result":super::skill_read_result(&output, 16000)}),
	);
	let full_growth = aidash_domain::context::tool_event_growth(&context, &full_event);
	let minimum_event = typed_event(
		serde_json::json!({"kind":"tool","call":call,"result":super::skill_read_result(&output, 0)}),
	);
	let minimum_growth = aidash_domain::context::tool_event_growth(&context, &minimum_event);
	assert!(full_growth > minimum_growth);
	let request_window = 100_000;
	let request_tokens = request_window - (full_growth + minimum_growth) / 2;
	let budget = super::WorkspaceReadFitBudget {
		requested: 16000,
		offset: 0,
		request_tokens,
		request_window,
		remaining_calls: 0,
	};
	let bytes = super::fit_skill_read_chars(&context, &call, &output, budget).unwrap();
	assert!(bytes > 0 && bytes < 15000);
	let result = super::skill_read_result(&output, bytes);
	let end = result["next_offset"].as_u64().unwrap() as usize;
	assert_eq!(end, result["text"].as_str().unwrap().len());
	assert_eq!(end % 3, 0);
	assert_eq!(result["budget_limited"], true);
	let mut bounded_call = call.clone();
	bounded_call.arguments["max_chars"] = serde_json::json!(bytes);
	let event = typed_event(serde_json::json!({"kind":"tool","call":bounded_call,"result":result}));
	assert!(
		request_tokens + aidash_domain::context::tool_event_growth(&context, &event)
			<= request_window
	);
	assert!(
		super::fit_skill_read_chars(
			&context,
			&call,
			&output,
			super::WorkspaceReadFitBudget {
				request_tokens: request_window,
				..budget
			}
		)
		.is_none()
	);
	assert_eq!(super::skill_read_result(&output, 0)["deferred"], true);
}

#[rstest::rstest]

fn skill_read_can_fit_one_character_when_the_deferred_envelope_cannot_fit() {
	let call = aidash_domain::provider::ToolCall {
		id: "skill-1".into(),
		name: "skill_read".into(),
		arguments: serde_json::json!({"skill":{"id":"research","version":"1.0.0"},"path":"references/guide.md","offset":0,"max_chars":1}),
	};
	let context = aidash_domain::context::Context::default();
	let output = serde_json::json!({"path":"references/guide.md","text":"界more","encoding":"utf8","offset":0,"total_chars":7,"next_offset":null});
	let event_growth = |bytes| {
		let mut bounded_call = call.clone();
		bounded_call.arguments["max_chars"] = serde_json::json!(bytes);
		let event = typed_event(serde_json::json!({
			"kind":"tool",
			"call":bounded_call,
			"result":super::skill_read_result(&output, bytes)
		}));
		aidash_domain::context::tool_event_growth(&context, &event)
	};
	let deferred_growth = event_growth(0);
	let one_character_growth = event_growth(3);
	assert_eq!(super::skill_read_result(&output, 1)["text"], "界");
	assert!(deferred_growth > one_character_growth);

	let request_window = 10_000;
	let request_tokens = request_window - one_character_growth;
	assert!(request_tokens + deferred_growth > request_window);
	let bytes = super::fit_skill_read_chars(
		&context,
		&call,
		&output,
		super::WorkspaceReadFitBudget {
			requested: 1,
			offset: 0,
			request_tokens,
			request_window,
			remaining_calls: 0,
		},
	)
	.unwrap();
	assert_eq!(bytes, 3);
	assert_eq!(super::skill_read_result(&output, bytes)["text"], "界");
}

#[rstest::rstest]

fn workspace_read_chunks_fit_remaining_complete_request_budget() {
	let call = aidash_domain::provider::ToolCall {
		id: "read-1".into(),
		name: "workspace_read".into(),
		arguments: serde_json::json!({
			"kind":"artifact",
			"id":"00000000-0000-0000-0000-000000000001",
			"offset":0,
			"max_chars":16000
		}),
	};
	let context = aidash_domain::context::Context::default();
	let record = serde_json::json!({"content":"界".repeat(20000)});
	let output = aidash_domain::context::observation::chunk_record(
		record,
		"artifact",
		"00000000-0000-0000-0000-000000000001",
		0,
		16000,
	)
	.unwrap();
	let window = 100_000;
	let request_window = super::request_context_window(window, 4_000);
	let allowed = request_window - 2 * super::TOOL_EVENT_RESERVE;
	let request_tokens = request_window - 4_000;
	let chars = super::fit_workspace_read_chars(
		&context,
		&call,
		&output,
		super::WorkspaceReadFitBudget {
			requested: 16_000,
			offset: 0,
			request_tokens,
			request_window,
			remaining_calls: 2,
		},
	)
	.unwrap()
	.unwrap();
	assert!(chars > 0 && chars < 16000);
	let mut bounded_call = call.clone();
	bounded_call.arguments["max_chars"] = serde_json::json!(chars);
	let result = super::workspace_read_result(&output, 16000, 0, chars);
	let event = typed_event(serde_json::json!({"kind":"tool","call":bounded_call,"result":result}));
	assert!(
		request_tokens + aidash_domain::context::tool_event_growth(&context, &event) <= allowed
	);
	let mut old_quota_call = call.clone();
	old_quota_call.arguments["max_chars"] = serde_json::json!(chars + 1);
	let old_quota_result = super::workspace_read_result(&output, 16000, 0, chars + 1);
	let old_quota_event = typed_event(
		serde_json::json!({"kind":"tool","call":old_quota_call,"result":old_quota_result}),
	);
	let old_hard_window_quota =
		window - super::POST_TOOL_CONTEXT_RESERVE - 2 * super::TOOL_EVENT_RESERVE;
	assert!(
		request_tokens + aidash_domain::context::tool_event_growth(&context, &old_quota_event)
			<= old_hard_window_quota
	);
	assert!(
		request_tokens + aidash_domain::context::tool_event_growth(&context, &old_quota_event)
			> allowed
	);
	let mut too_large = call;
	too_large.arguments["max_chars"] = serde_json::json!(chars + 1);
	let result = super::workspace_read_result(&output, 16000, 0, chars + 1);
	let event = typed_event(serde_json::json!({"kind":"tool","call":too_large,"result":result}));
	assert!(request_tokens + aidash_domain::context::tool_event_growth(&context, &event) > allowed);
}

#[rstest::rstest]

fn workspace_read_fit_uses_the_persisted_request_window() {
	let call = aidash_domain::provider::ToolCall {
		id: "read-1".into(),
		name: "workspace_read".into(),
		arguments: serde_json::json!({
			"kind":"artifact",
			"id":"00000000-0000-0000-0000-000000000001",
			"offset":0,
			"max_chars":16000
		}),
	};
	let context = aidash_domain::context::Context::default();
	let output = aidash_domain::context::observation::chunk_record(
		serde_json::json!({"content":"界".repeat(20000)}),
		"artifact",
		"00000000-0000-0000-0000-000000000001",
		0,
		16000,
	)
	.unwrap();
	let hard_window = 32_000;
	let request_window = super::request_context_window(hard_window, 8_000);
	let request_tokens = request_window - 2_000;
	let chars = super::fit_workspace_read_chars(
		&context,
		&call,
		&output,
		super::WorkspaceReadFitBudget {
			requested: 16_000,
			offset: 0,
			request_tokens,
			request_window,
			remaining_calls: 0,
		},
	)
	.unwrap()
	.expect("the persisted request quota is used without a duplicate reserve");
	assert!(chars > 0);
}

#[rstest::rstest]

fn workspace_observation_fit_includes_the_adjusted_call_and_following_events() {
	let context = aidash_domain::context::Context::default();
	let call = aidash_domain::provider::ToolCall {
		id: "observe-1".into(),
		name: "workspace_observe".into(),
		arguments: serde_json::json!({"offset":0,"limit":20}),
	};
	let output = serde_json::json!({
		"view":"workspace_observation_v1",
		"tasks":[{"description_preview":"x".repeat(512),"dependencies":vec!["00000000-0000-0000-0000-000000000001";50]}],
		"pages":{"tasks":{"limit":1}}
	});
	let request_window = 32_000;
	let remaining_calls = 2;
	let target = request_window - remaining_calls * super::TOOL_EVENT_RESERVE;
	let growth = aidash_domain::context::tool_event_growth(
		&context,
		&typed_event(serde_json::json!({
			"kind":"tool",
			"call":{"id":"observe-1","name":"workspace_observe","arguments":{"offset":0,"limit":1}},
			"result":output
		})),
	);
	assert!(super::workspace_observation_event_fits(
		&context,
		&call,
		1,
		&output,
		target - growth,
		request_window,
		remaining_calls
	));
	assert!(!super::workspace_observation_event_fits(
		&context,
		&call,
		1,
		&output,
		target - growth + 1,
		request_window,
		remaining_calls
	));
}

#[rstest::rstest]

fn workspace_read_rejects_negative_offset_before_fitting() {
	let call = aidash_domain::provider::ToolCall {
		id: "read-1".into(),
		name: "workspace_read".into(),
		arguments: serde_json::json!({"kind":"event","id":"event-id","offset":-1}),
	};
	assert!(matches!(
		super::workspace_read_range(&call),
		Err(crate::Error::Invalid(message)) if message.contains("offset")
	));
}

#[rstest::rstest]

fn workspace_read_rejects_oversized_max_chars_before_fitting() {
	let call = aidash_domain::provider::ToolCall {
		id: "read-1".into(),
		name: "workspace_read".into(),
		arguments: serde_json::json!({"kind":"event","id":"event-id","max_chars":16001}),
	};
	assert!(matches!(
		super::workspace_read_range(&call),
		Err(crate::Error::Invalid(message)) if message.contains("16000")
	));
}

#[rstest::rstest]

fn cached_workspace_read_plans_still_validate_the_original_call() {
	let pending = pending(serde_json::json!({
		"workspace_read_plan": {
			"step": 4,
			"cursor": 0,
			"call":{"id":"read-1","name":"workspace_read","arguments":{}},
			"result": {"content":"previously prepared"}
		}
	}));
	let mut call = aidash_domain::provider::ToolCall {
		id: "read-1".into(),
		name: "workspace_read".into(),
		arguments: serde_json::json!({"kind":"event","id":"event-id","offset":-1}),
	};
	assert!(matches!(
		super::workspace_read_plan_result(&call, 4, 0, &pending),
		Err(crate::Error::Invalid(message)) if message.contains("offset")
	));

	call.arguments["offset"] = serde_json::json!(0);
	call.arguments["max_chars"] = serde_json::json!(16001);
	assert!(matches!(
		super::workspace_read_plan_result(&call, 4, 0, &pending),
		Err(crate::Error::Invalid(message)) if message.contains("16000")
	));
}

#[rstest::rstest]

fn zero_length_envelope_is_checked_before_deferring_a_read() {
	let call = aidash_domain::provider::ToolCall {
		id: "read-1".into(),
		name: "workspace_read".into(),
		arguments: serde_json::json!({
			"kind":"artifact",
			"id":"00000000-0000-0000-0000-000000000001",
			"offset":0,
			"max_chars":16000
		}),
	};
	let context = aidash_domain::context::Context::default();
	let record = serde_json::json!({"content":"界".repeat(20000)});
	let output = aidash_domain::context::observation::chunk_record(
		record,
		"artifact",
		"00000000-0000-0000-0000-000000000001",
		0,
		16000,
	)
	.unwrap();
	let zero = super::workspace_read_result(&output, 16000, 0, 0);
	assert_eq!(zero["deferred"], true);
	assert!(
		super::fit_workspace_read_chars(
			&context,
			&call,
			&output,
			super::WorkspaceReadFitBudget {
				requested: 16_000,
				offset: 0,
				request_tokens: 0,
				request_window: 1,
				remaining_calls: 0,
			},
		)
		.unwrap()
		.is_none()
	);
	let next_window = super::force_workspace_read_compaction_window(32_000, 8_000);
	assert_eq!(next_window, 32_000 - super::POST_TOOL_CONTEXT_RESERVE);
	assert!(next_window < 32_000);
	let deferred = super::deferred_workspace_read(&call);
	assert_eq!(json!(deferred.call)["arguments"]["offset"], 0);
	assert_eq!(
		json!(deferred.call)["arguments"]["id"],
		call.arguments["id"]
	);
}

#[rstest::rstest]

fn referenced_run_message_requires_every_record_chunk() {
	let id = uuid::Uuid::new_v4();
	let event = |offset: usize, content: &str, next: Option<usize>| {
		typed_event(serde_json::json!({
			"kind":"tool",
			"call":{"id":"read-message","name":"workspace_read","arguments":{"kind":"message","id":id}},
			"result":{"kind":"message","id":id,"encoding":"json","offset":offset,"total_chars":6,"content":content,"next_offset":next}
		}))
	};
	let mut context = aidash_domain::context::Context::default();
	context.history.push(event(0, "abc", Some(3)));
	super::capture_message_read_coverage(&mut context);
	assert!(!super::referenced_message_read(&context, id));
	context.history.push(event(4, "ef", None));
	super::capture_message_read_coverage(&mut context);
	assert!(!super::referenced_message_read(&context, id));
	context.history.push(event(3, "def", None));
	super::capture_message_read_coverage(&mut context);
	assert!(super::referenced_message_read(&context, id));
	context.history.clear();
	assert!(super::referenced_message_read(&context, id));
	// Completed reads alone are not evidence that compaction left their
	// content in a provider request.
	super::capture_message_inference_coverage(&mut context);
	assert!(!super::referenced_message_inferred(&context, id));
	context.history.push(event(0, "abc", Some(3)));
	super::capture_message_inference_coverage(&mut context);
	assert!(!super::referenced_message_inferred(&context, id));
	context.history.clear();
	context.history.push(event(3, "def", None));
	super::capture_message_inference_coverage(&mut context);
	assert!(super::referenced_message_inferred(&context, id));
	context.history.clear();
	assert!(super::referenced_message_inferred(&context, id));
}

#[rstest::rstest]

fn result_names_fit_for_ascii_and_multibyte_titles() {
	for title in ["a".repeat(64_000), "界".repeat(21_333)] {
		let name = super::result_artifact_name(&title);
		assert!(name.len() <= 64_000);
		assert!(name.ends_with(" result"));
	}
}
