use super::*;
#[rstest::rstest]
fn media_reservation_covers_complete_request_growth() {
	let mut request = ModelRequest {
		instructions: "Inspect the media".into(),
		context: json!({"history":"quoted \\\"text\\\" and 日本語"}),
		tools: vec![],
		max_output_tokens: 512,
		response_format: None,
		content_parts: vec![],
	};
	let without_media = request.estimated_total_tokens();
	request.content_parts = vec![
		ContentPart::Text("attachment: \\\"sample\\\"".into()),
		ContentPart::Image {
			media_type: "image/png".into(),
			bytes: vec![0; 8192],
		},
		ContentPart::Audio {
			format: "wav".into(),
			bytes: vec![0; 32_768],
		},
	];
	assert!(
		request.estimated_total_tokens()
			<= without_media.saturating_add(ModelRequest::content_parts_reservation(
				&request.content_parts
			))
	);
}

#[rstest::rstest]
fn parses_openrouter_tool_calls() {
	let result = parse_openai(json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"one","function":{"name":"search","arguments":"{\"q\":\"Rust\"}"}}]}}]})).unwrap();
	assert_eq!(result.tool_calls[0].arguments, json!({"q":"Rust"}));
}
#[rstest::rstest]
fn whitespace_only_tool_call_content_is_not_a_workspace_message() {
	let result = parse_openai(json!({"choices":[{"finish_reason":"tool_calls","message":{"content":" \n\t ","tool_calls":[{"id":"one","function":{"name":"search","arguments":"{}"}}]}}]})).unwrap();
	assert!(result.text.is_empty());
	assert_eq!(result.tool_calls.len(), 1);
	assert!(
		parse_openai(json!({"choices":[{"finish_reason":"stop","message":{"content":" \n\t "}}]}))
			.is_err()
	);
}
#[rstest::rstest]
fn truncation_cannot_complete_a_task() {
	assert!(matches!(
		parse_openai(
			json!({"choices":[{"finish_reason":"length","message":{"content":"partial"}}]})
		),
		Err(Error::Context(Failure::OutputTruncated))
	));
}
#[rstest::rstest]
#[case::length("length", Failure::OutputTruncated)]
#[case::content_filter("content_filter", Failure::Refused)]
fn truncated_or_filtered_output_never_yields_tool_calls(
	#[case] finish_reason: &str,
	#[case] expected: Failure,
) {
	let result = parse_openai(json!({"choices":[{"finish_reason":finish_reason,"message":{
		"tool_calls":[{"id":"one","function":{"name":"search","arguments":"{}"}}]}}]}));
	assert!(matches!(result, Err(Error::Context(actual)) if actual == expected));
}
#[rstest::rstest]
#[case::stop("stop")]
#[case::tool_calls("tool_calls")]
fn a_refusal_is_typed_whatever_the_finish_reason(#[case] finish_reason: &str) {
	let result = parse_openai(json!({"choices":[{"finish_reason":finish_reason,"message":{
		"refusal":"I can't help with that",
		"tool_calls":[{"id":"one","function":{"name":"search","arguments":"{}"}}]}}]}));
	assert!(matches!(result, Err(Error::Context(Failure::Refused))));
}
#[rstest::rstest]
fn other_unexpected_finish_reasons_remain_external() {
	assert!(matches!(
		parse_openai(json!({"choices":[{"finish_reason":"error","message":{"content":"x"}}]})),
		Err(Error::External(_))
	));
}
#[rstest::rstest]
#[case::openrouter(400, json!({"error":{"code":400,"message":"This endpoint's maximum context length is 8192 tokens. However, you requested about 9001 tokens (8000 of text input, 1001 in the output). Please reduce the length of either one."}}))]
#[case::code(400, json!({"error":{"code":"context_length_exceeded","message":"Input is too large"}}))]
#[case::message(400, json!({"error":{"message":"context_length_exceeded"}}))]
#[case::anthropic(400, json!({"error":{"code":400,"message":"Provider returned error","metadata":{"raw":"prompt is too long: 210000 tokens > 200000 maximum"}}}))]
#[case::payload_too_large(413, json!({"error":{"message":"prompt is too long"}}))]
fn provider_proven_overflow_is_typed(#[case] status: u16, #[case] body: Value) {
	for byok in [false, true] {
		assert!(matches!(
			rejection(status, Some(&body), byok),
			Error::ContextOverflow
		));
	}
}
#[rstest::rstest]
#[case::media(Some(json!({"error":{"message":"Audio exceeds the provider limit"}})), "Audio exceeds the provider limit")]
#[case::entity(Some(json!({"error":{"message":"Request Entity Too Large"}})), "upstream rejected the request")]
#[case::unreadable(None, "upstream rejected the request")]
fn payload_too_large_without_proof_is_not_an_overflow(
	#[case] body: Option<Value>,
	#[case] expected: &str,
) {
	assert!(matches!(
		rejection(413, body.as_ref(), false),
		Error::ProviderRejected { status: 413, reason } if reason == expected
	));
}
#[rstest::rstest]
#[case::invalid(400, Some(json!({"error":{"message":"Invalid tool schema: secret detail"}})))]
#[case::unreadable(400, None)]
#[case::overflow_wording_on_other_status(401, Some(json!({"error":{"message":"maximum context length is 10 tokens"}})))]
fn unrelated_rejections_stay_provider_rejected_without_the_body(
	#[case] status: u16,
	#[case] body: Option<Value>,
) {
	let error = rejection(status, body.as_ref(), false);
	assert!(
		matches!(&error, Error::ProviderRejected { status: actual, reason } if *actual == status && reason == "upstream rejected the request"),
		"{error:?}"
	);
	assert!(!error.to_string().contains("secret"));
}
#[rstest::rstest]
fn byok_chat_keeps_broker_capability_failures_typed() {
	let body = json!({"error":{"code":"capability_expired","message":"maximum context length"}});
	assert!(matches!(
		rejection(401, Some(&body), true),
		Error::Invalid(message) if message.contains("capability_expired")
	));
	assert!(matches!(
		rejection(401, Some(&body), false),
		Error::ProviderRejected { status: 401, .. }
	));
}
#[rstest::rstest]
#[case::text(vec![])]
#[case::media(vec!["google-vertex".to_owned()])]
fn every_chat_request_disables_openrouter_transforms(#[case] routes: Vec<String>) {
	let config: ModelConfig = serde_json::from_value(json!({
		"provider":"openrouter","model_id":"vendor/model","endpoint":"https://openrouter.ai/api/v1",
		"credential_env":null,"context_window":8192,"modalities":["text"],"cost":{}
	}))
	.unwrap();
	let request = ModelRequest {
		instructions: "Answer".into(),
		context: json!({"history":[]}),
		tools: vec![],
		max_output_tokens: 256,
		response_format: None,
		content_parts: vec![],
	};
	let body = request_body(&config, &request, routes);
	assert_eq!(body["transforms"], json!([]));
	assert_eq!(body["model"], json!("vendor/model"));
	assert_eq!(body["provider"]["zdr"], json!(true));
}
