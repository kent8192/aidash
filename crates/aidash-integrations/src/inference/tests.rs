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
	let result = parse(json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"one","function":{"name":"search","arguments":"{\"q\":\"Rust\"}"}}]}}]})).unwrap();
	assert_eq!(result.tool_calls[0].arguments, json!({"q":"Rust"}));
}
#[rstest::rstest]
fn whitespace_only_tool_call_content_is_not_a_workspace_message() {
	let result = parse(json!({"choices":[{"finish_reason":"tool_calls","message":{"content":" \n\t ","tool_calls":[{"id":"one","function":{"name":"search","arguments":"{}"}}]}}]})).unwrap();
	assert!(result.text.is_empty());
	assert_eq!(result.tool_calls.len(), 1);
	assert!(
		parse(json!({"choices":[{"finish_reason":"stop","message":{"content":" \n\t "}}]}))
			.is_err()
	);
}
#[rstest::rstest]
fn truncation_cannot_complete_a_task() {
	assert!(
		parse(json!({"choices":[{"finish_reason":"length","message":{"content":"partial"}}]}))
			.is_err()
	);
}

fn parse(value: Value) -> Result<ModelResponse> {
	parse_openai(value.to_string().as_bytes())
}

fn completion(usage: Value) -> Value {
	json!({"choices":[{"finish_reason":"stop","message":{"content":"done"}}],"usage":usage})
}

#[rstest::rstest]
#[case::complete(
	json!({"prompt_tokens":120,"completion_tokens":30,"total_tokens":150}),
	ReportedUsage { input_tokens: Some(120), output_tokens: Some(30), ..Default::default() },
	(120, 30, true)
)]
#[case::missing_prompt(
	json!({"completion_tokens":30}),
	ReportedUsage { output_tokens: Some(30), ..Default::default() },
	(0, 30, false)
)]
#[case::missing_completion(
	json!({"prompt_tokens":120}),
	ReportedUsage { input_tokens: Some(120), ..Default::default() },
	(120, 0, false)
)]
#[case::invalid_counts(
	json!({"prompt_tokens":-1,"completion_tokens":"30","prompt_tokens_details":{"cached_tokens":1.5}}),
	ReportedUsage::default(),
	(0, 0, false)
)]
#[case::cache_read_and_write(
	json!({"prompt_tokens":2000,"completion_tokens":40,"prompt_tokens_details":{"cached_tokens":1500,"cache_write_tokens":400},"completion_tokens_details":{"reasoning_tokens":12}}),
	ReportedUsage { input_tokens: Some(2000), output_tokens: Some(40), cache_read_tokens: Some(1500), cache_write_tokens: Some(400), reasoning_tokens: Some(12), cost: None },
	(2000, 40, true)
)]
#[case::cache_absent(
	json!({"prompt_tokens":2000,"completion_tokens":40,"prompt_tokens_details":{},"completion_tokens_details":null}),
	ReportedUsage { input_tokens: Some(2000), output_tokens: Some(40), ..Default::default() },
	(2000, 40, true)
)]
#[case::cost_present(
	json!({"prompt_tokens":10,"completion_tokens":5,"cost":0.0001234,"cost_details":{"upstream_inference_cost":0.0001}}),
	ReportedUsage { input_tokens: Some(10), output_tokens: Some(5), cost: Some(ProviderCost { nanocredits: 123_400, upstream_nanocredits: Some(100_000) }), ..Default::default() },
	(10, 5, true)
)]
#[case::cost_absent(
	json!({"prompt_tokens":10,"completion_tokens":5,"cost_details":{"upstream_inference_cost":0.0001}}),
	ReportedUsage { input_tokens: Some(10), output_tokens: Some(5), ..Default::default() },
	(10, 5, true)
)]
#[case::cost_exponent(
	json!({"prompt_tokens":10,"completion_tokens":5,"cost":1.5e-7,"cost_details":{"upstream_inference_cost":null}}),
	ReportedUsage { input_tokens: Some(10), output_tokens: Some(5), cost: Some(ProviderCost { nanocredits: 150, upstream_nanocredits: None }), ..Default::default() },
	(10, 5, true)
)]
#[case::cost_invalid(
	json!({"prompt_tokens":10,"completion_tokens":5,"cost":"0.1"}),
	ReportedUsage { input_tokens: Some(10), output_tokens: Some(5), ..Default::default() },
	(10, 5, true)
)]
fn openrouter_usage_keeps_unknown_fields_absent(
	#[case] usage: Value,
	#[case] reported: ReportedUsage,
	#[case] legacy: (u64, u64, bool),
) {
	let result = parse(completion(usage)).unwrap();
	assert_eq!(result.reported, reported);
	assert_eq!(
		(
			result.input_tokens,
			result.output_tokens,
			result.usage_complete
		),
		legacy
	);
}

#[rstest::rstest]
fn missing_usage_object_is_unknown() {
	let result =
		parse(json!({"choices":[{"finish_reason":"stop","message":{"content":"done"}}]})).unwrap();
	assert_eq!(result.reported, ReportedUsage::default());
	assert!(!result.usage_complete);
}

/// Costs keep their raw decimal text: rounding through `f64` first would turn
/// `0.1234567890000000000000000001` into `0.123456789` and understate the cost.
#[rstest::rstest]
#[case::cost_beyond_f64_precision(
	r#"{"choices":[{"finish_reason":"stop","message":{"content":"done"}}],"usage":{"prompt_tokens":1,"completion_tokens":1,"cost":0.1234567890000000000000000001,"cost_details":{"upstream_inference_cost":1.0000000000000000000000000001e-9}}}"#,
	Some(ProviderCost { nanocredits: 123_456_790, upstream_nanocredits: Some(2) })
)]
#[case::unexpected_cost_shape(
	r#"{"choices":[{"finish_reason":"stop","message":{"content":"done"}}],"usage":{"prompt_tokens":1,"completion_tokens":1,"cost":{"total":1},"cost_details":"none"}}"#,
	None
)]
fn openrouter_cost_uses_the_raw_decimal(#[case] body: &str, #[case] cost: Option<ProviderCost>) {
	let result = parse_openai(body.as_bytes()).unwrap();
	assert_eq!(result.reported.cost, cost);
	assert!(result.usage_complete);
}

/// Usage shape OpenRouter returns for an explicit `cache_control` request on an
/// Anthropic route: the cache counts are breakdowns of `prompt_tokens`.
#[rstest::rstest]
#[case::cache_write(json!({"prompt_tokens":4120,"completion_tokens":96,"total_tokens":4216,"cost":0.0277,"prompt_tokens_details":{"cached_tokens":0,"cache_write_tokens":4096},"completion_tokens_details":{"reasoning_tokens":0},"cost_details":{"upstream_inference_cost":0.0277}}), 4216)]
#[case::cache_read(json!({"prompt_tokens":4120,"completion_tokens":88,"total_tokens":4208,"cost":0.00268,"prompt_tokens_details":{"cached_tokens":4096,"cache_write_tokens":0},"completion_tokens_details":{"reasoning_tokens":0},"cost_details":{"upstream_inference_cost":0.00268}}), 4208)]
fn explicit_cache_settlement_charges_prompt_and_completion_once(
	#[case] usage: Value,
	#[case] total: i64,
) {
	let response = parse(completion(usage)).unwrap();
	let accounting = aidash_domain::generation::inference::accounting(8192, &response);
	assert_eq!(accounting.reported, Some(total));
	assert_eq!(accounting.refund, 8192 - total);
	assert!(!accounting.exceeded);
	assert_eq!(
		aidash_domain::generation::inference::complete_reported_usage(&response),
		Some(total)
	);
}
