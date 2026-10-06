use super::*;
#[rstest::rstest]
fn media_reservation_covers_complete_request_growth() {
	let mut request = ModelRequest {
		instructions: "Inspect the media".into(),
		context: json!({"history":"quoted \\\"text\\\" and 日本語"}),
		tools: vec![],
		max_output_tokens: 512,
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
	assert!(
		parse_openai(
			json!({"choices":[{"finish_reason":"length","message":{"content":"partial"}}]})
		)
		.is_err()
	);
}
