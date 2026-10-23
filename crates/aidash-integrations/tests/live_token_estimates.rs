//! Live checks of tokenized Request Estimates (#276, ADR 0017). They send
//! fixture text to OpenRouter and assert that the input side of each estimate
//! is at least the provider's reported `prompt_tokens`. That is the bound a
//! Token Encoding must keep, and it is how the message and tool framing
//! constants were set. They are ignored by default and need a key:
//!
//!     OPENROUTER_API_KEY=... cargo test -p aidash-integrations \
//!         --test live_token_estimates -- --ignored --nocapture
//!
//! `AIDASH_LIVE_OPENAI_MODEL` and `AIDASH_LIVE_GEMINI_MODEL` override the
//! routes.
use aidash_domain::provider::estimate::encoding::{MESSAGE_TOKENS, TOOL_TOKENS};
use aidash_domain::provider::estimate::{EstimateSnapshot, FRAMING_RESERVE, TokenEncoding};
use aidash_domain::provider::{ModelRequest, ToolSpec};
use rstest::rstest;
use serde_json::{Value, json};

const OUTPUT_TOKENS: u32 = 256;

fn route(encoding: TokenEncoding) -> String {
	let (variable, default) = match encoding {
		TokenEncoding::O200kBase => ("AIDASH_LIVE_OPENAI_MODEL", "openai/gpt-5.4-nano"),
		TokenEncoding::Gemma4 => ("AIDASH_LIVE_GEMINI_MODEL", "google/gemini-3.1-flash-lite"),
	};
	std::env::var(variable).unwrap_or_else(|_| default.into())
}

/// The provider's reported prompt tokens for `body` on `model`.
async fn reported_prompt_tokens(model: &str, body: &Value) -> u64 {
	let key = std::env::var("OPENROUTER_API_KEY").expect("OPENROUTER_API_KEY is required");
	let mut request = body.clone();
	request["model"] = json!(model);
	request["max_tokens"] = json!(OUTPUT_TOKENS);
	request["stream"] = json!(false);
	let response = reqwest::Client::new()
		.post("https://openrouter.ai/api/v1/chat/completions")
		.bearer_auth(key)
		.json(&request)
		.send()
		.await
		.unwrap();
	let status = response.status();
	let answer: Value = response.json().await.unwrap();
	assert!(status.is_success(), "{model}: {status} {answer}");
	answer["usage"]["prompt_tokens"]
		.as_u64()
		.unwrap_or_else(|| panic!("{model}: no prompt_tokens in {answer}"))
}

fn tool(index: usize) -> Value {
	json!({"type":"function","function":{
		"name": format!("tool_{index}"),
		"description": format!("Reads record {index} from the workspace."),
		"parameters": {"type":"object","properties":{
			"id":{"type":"string","description":"Record id"},
			"limit":{"type":"integer","description":"Most lines to return"}
		},"required":["id"]}
	}})
}

/// Bodies that stress one part of the estimate each.
fn bodies() -> Vec<(&'static str, Value)> {
	let many_messages = (0..60)
		.map(|index| {
			let role = if index % 2 == 0 { "user" } else { "assistant" };
			json!({"role": role, "content": if index % 3 == 0 { "ok" } else { "東京" }})
		})
		.collect::<Vec<_>>();
	let mut many_messages = many_messages;
	many_messages.insert(0, json!({"role":"system","content":"Answer briefly."}));
	many_messages.push(json!({"role":"user","content":"Say done."}));
	vec![
		("many_messages", json!({"messages": many_messages})),
		(
			"many_tools",
			json!({
				"messages": [{"role":"system","content":"Answer briefly."},{"role":"user","content":"Say done."}],
				"tools": (0..40).map(tool).collect::<Vec<_>>(),
			}),
		),
	]
}

/// A complete request as the harness builds it: instructions, an escaped
/// multilingual context and a large tool schema.
fn request(encoding: TokenEncoding) -> ModelRequest {
	let properties = (0..120)
		.map(|index| {
			(
				format!("field_{index}"),
				json!({"type":"string","description":format!("説明 {index} with \"quotes\"")}),
			)
		})
		.collect::<serde_json::Map<_, _>>();
	ModelRequest {
		instructions: "Follow the task. Reply with the word done.".into(),
		context: json!({
			"task": "Summarize the notes.",
			"notes": "東京都の天気は晴れです。한국어 텍스트. Ünïcödé. ".repeat(40),
			"escaped": "{\"nested\":\"line\\nbreak\\t\"}",
		})
		.into(),
		tools: vec![ToolSpec {
			name: "large".into(),
			description: "A large schema".into(),
			parameters: json!({"type":"object","properties":properties}),
		}],
		max_output_tokens: OUTPUT_TOKENS,
		response_format: None,
		content_parts: vec![],
		cache_scope: None,
		cache_breakpoints: false,
		disable_provider_transforms: false,
		input_format: Default::default(),
		estimate: EstimateSnapshot::tokenized(0, encoding, None),
	}
}

#[rstest]
#[case::openai(TokenEncoding::O200kBase)]
#[case::gemini(TokenEncoding::Gemma4)]
#[ignore = "live: sends fixture text to OpenRouter and needs OPENROUTER_API_KEY"]
#[tokio::test]
async fn tokenized_estimates_bound_reported_prompt_tokens(#[case] encoding: TokenEncoding) {
	let model = route(encoding);
	println!(
		"{} on {model}: MESSAGE_TOKENS={MESSAGE_TOKENS} TOOL_TOKENS={TOOL_TOKENS}",
		encoding.name()
	);
	for (name, body) in bodies() {
		let estimate = encoding.count_body(&body).unwrap() + FRAMING_RESERVE;
		let reported = reported_prompt_tokens(&model, &body).await;
		println!("  {name}: estimate {estimate}, reported {reported}");
		assert!(
			estimate as u64 >= reported,
			"{name}: {estimate} < {reported}"
		);
	}
	let request = request(encoding);
	let estimate = request.request_estimate();
	let input = estimate.tokens - estimate.output_reservation;
	let reported = reported_prompt_tokens(&model, &request.input_body()).await;
	println!(
		"  request: estimate {input}, reported {reported}, bytes {}",
		request.byte_estimate().basis
	);
	assert_eq!(estimate.estimator, "tokenized");
	assert!(input >= reported, "request: {input} < {reported}");
}
