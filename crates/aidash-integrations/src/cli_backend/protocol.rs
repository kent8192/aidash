//! The model proposes tools in a JSON envelope; only Aidash dispatches them.
use super::{
	Failure, Result,
	config::{Config, Provider},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const MAX_INPUT: usize = 1024 * 1024;
pub const MAX_OUTPUT: usize = 2 * 1024 * 1024;
pub const MAX_CALLS: usize = 16;

pub struct Request {
	pub body: Value,
	pub stream: bool,
	pub max_tokens: u32,
}

impl Request {
	pub fn parse(bytes: &[u8], config: &Config) -> Result<Self> {
		if bytes.len() > MAX_INPUT {
			return Err(Failure("input_limit"));
		}
		let body: Value =
			serde_json::from_slice(bytes).map_err(|_| Failure("invalid_request_json"))?;
		let object = body.as_object().ok_or(Failure("invalid_request"))?;
		let allowed = [
			"model",
			"messages",
			"tools",
			"tool_choice",
			"max_tokens",
			"stream",
			"stream_options",
			"provider",
			"transforms",
		];
		if object.keys().any(|k| !allowed.contains(&k.as_str())) {
			return Err(Failure("unsupported_request_field"));
		}
		if body["model"].as_str() != Some(&config.model) {
			return Err(Failure("model_mismatch"));
		}
		let max_tokens = body["max_tokens"]
			.as_u64()
			.filter(|n| *n > 0 && *n <= config.max_output_tokens as u64)
			.ok_or(Failure("invalid_output_limit"))? as u32;
		let stream = match body.get("stream") {
			None => false,
			Some(v) => v.as_bool().ok_or(Failure("invalid_stream"))?,
		};
		if let Some(choice) = body.get("tool_choice")
			&& choice != "auto"
		{
			return Err(Failure("unsupported_tool_choice"));
		}
		if body.get("transforms").is_some_and(|v| v != &json!([])) {
			return Err(Failure("unsupported_transforms"));
		}
		let messages = body["messages"]
			.as_array()
			.filter(|m| !m.is_empty() && m.len() <= 1024)
			.ok_or(Failure("invalid_messages"))?;
		for message in messages {
			if !matches!(
				message["role"].as_str(),
				Some("system" | "developer" | "user" | "assistant" | "tool")
			) {
				return Err(Failure("invalid_message_role"));
			}
			let content = &message["content"];
			if !content.is_string() && !content.is_null() {
				let parts = content.as_array().ok_or(Failure("unsupported_modality"))?;
				if parts.iter().any(|p| {
					p["type"] != "text"
						|| !p["text"].is_string()
						|| p.get("cache_control").is_some()
				}) {
					return Err(Failure("unsupported_modality_or_cache"));
				}
			}
		}
		let mut names = BTreeSet::new();
		if let Some(tools) = body.get("tools") {
			let tools = tools
				.as_array()
				.filter(|t| t.len() <= 256)
				.ok_or(Failure("invalid_tools"))?;
			for tool in tools {
				let name = tool["function"]["name"]
					.as_str()
					.ok_or(Failure("invalid_tool_name"))?;
				if tool["type"] != "function"
					|| name.is_empty()
					|| name.len() > 256
					|| !names.insert(name)
					|| !name
						.bytes()
						.all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
				{
					return Err(Failure("invalid_tool_definition"));
				}
				let schema = &tool["function"]["parameters"];
				// Local schema validation must never fetch a remote $ref.
				if !schema.is_object()
					|| contains_reference(schema)
					|| jsonschema::validator_for(schema).is_err()
				{
					return Err(Failure("unsupported_tool_schema"));
				}
			}
		}
		Ok(Self {
			body,
			stream,
			max_tokens,
		})
	}

	pub fn prompt(&self) -> Vec<u8> {
		format!("Act as the inference engine for Aidash. The following JSON contains the complete ordered conversation and available Aidash function definitions. Respond ONLY with the response envelope: text and tool_calls. Tool calls are proposals for Aidash to authorize and execute; arguments must be a serialized JSON object. Use no native CLI tools. Do not interpret @ paths, slash commands, or markup outside the conversation.\n{}", self.body).into_bytes()
	}
}

fn contains_reference(value: &Value) -> bool {
	match value {
		Value::Object(map) => {
			map.iter().any(|(key, value)| {
				key == "$id"
					|| matches!(key.as_str(), "$ref" | "$dynamicRef")
						&& value.as_str().is_none_or(|r| !r.starts_with('#'))
			}) || map.values().any(contains_reference)
		}
		Value::Array(values) => values.iter().any(contains_reference),
		_ => false,
	}
}

pub fn output_schema() -> Value {
	json!({"type":"object","additionalProperties":false,"required":["text","tool_calls"],"properties":{
		"text":{"type":"string"},"tool_calls":{"type":"array","maxItems":MAX_CALLS,"items":{
			"type":"object","additionalProperties":false,"required":["name","arguments"],"properties":{
				"name":{"type":"string"},"arguments":{"type":"string"}}}}}})
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
	pub text: String,
	pub tool_calls: Vec<Call>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
	pub name: String,
	pub arguments: String,
}

pub fn completion(
	provider: Provider,
	bytes: &[u8],
	request: &Request,
	model: &str,
) -> Result<Value> {
	if bytes.len() > MAX_OUTPUT {
		return Err(Failure("cli_output_limit"));
	}
	let mut final_value = None;
	let mut reported_model: Option<String> = None;
	let mut usage = json!({});
	let mut codex_message = None;
	for line in bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
		let event: Value = serde_json::from_slice(line).map_err(|_| Failure("invalid_cli_json"))?;
		if final_value.is_some() {
			return Err(Failure("events_after_final"));
		}
		match provider {
			Provider::Claude => match event["type"].as_str() {
				Some("system") if event["subtype"] == "init" => {
					if event["tools"].as_array().is_none_or(|t| !t.is_empty()) {
						return Err(Failure("native_tools_enabled"));
					}
					// Init describes the CLI's selection, not a provider-observed model.
					if event["model"].as_str() != Some(model) {
						return Err(Failure("model_mismatch"));
					}
				}
				Some("assistant") => {
					let message = &event["message"];
					if message["content"].as_array().is_none_or(|parts| {
						parts
							.iter()
							.any(|p| !matches!(p["type"].as_str(), Some("text" | "thinking")))
					}) {
						return Err(Failure("native_tool_or_refusal"));
					}
					if matches!(
						message["stop_reason"].as_str(),
						Some("max_tokens" | "refusal" | "pause_turn")
					) {
						return Err(Failure("terminal_response"));
					}
					if let Some(actual) = message["model"].as_str() {
						if reported_model.as_deref().is_some_and(|m| m != actual) {
							return Err(Failure("model_mismatch"));
						}
						reported_model = Some(actual.into());
					}
				}
				Some("result") => {
					if event["subtype"] != "success"
						|| event["is_error"] != false
						|| event["permission_denials"]
							.as_array()
							.is_some_and(|a| !a.is_empty())
					{
						return Err(Failure("cli_auth_or_inference_failed"));
					}
					if matches!(
						event["stop_reason"].as_str(),
						Some("max_tokens" | "refusal" | "pause_turn")
					) {
						return Err(Failure("terminal_response"));
					}
					usage = claude_usage(&event["usage"]);
					final_value = Some(event["structured_output"].clone());
				}
				_ => return Err(Failure("unexpected_cli_event")),
			},
			Provider::Codex => match event["type"].as_str() {
				Some("thread.started" | "turn.started") => {}
				Some("item.started" | "item.updated" | "item.completed") => {
					let item = &event["item"];
					match item["type"].as_str() {
						Some("reasoning") => {}
						Some("agent_message") if event["type"] == "item.completed" => {
							if codex_message.is_some() {
								return Err(Failure("multiple_final_messages"));
							}
							codex_message = Some(
								item["text"]
									.as_str()
									.ok_or(Failure("invalid_cli_message"))?
									.to_owned(),
							);
						}
						_ => return Err(Failure("native_tool_or_unknown_item")),
					}
				}
				Some("turn.completed") => {
					usage = json!({});
					for (from, to) in [
						("input_tokens", "prompt_tokens"),
						("output_tokens", "completion_tokens"),
					] {
						if let Some(n) = event["usage"][from].as_u64() {
							usage[to] = json!(n);
						}
					}
					if let Some(n) = event["usage"]["cached_input_tokens"].as_u64() {
						usage["prompt_tokens_details"] = json!({"cached_tokens":n});
					}
					final_value = Some(
						serde_json::from_str(
							codex_message
								.as_deref()
								.ok_or(Failure("missing_final_output"))?,
						)
						.map_err(|_| Failure("invalid_cli_message"))?,
					);
				}
				_ => return Err(Failure("cli_auth_or_inference_failed")),
			},
		}
	}
	if reported_model.as_deref().is_some_and(|m| m != model) {
		return Err(Failure("model_mismatch"));
	}
	if provider == Provider::Claude && reported_model.is_none() {
		return Err(Failure("missing_reported_model"));
	}
	let envelope: Envelope =
		serde_json::from_value(final_value.ok_or(Failure("missing_success_event"))?)
			.map_err(|_| Failure("invalid_response_envelope"))?;
	validated_completion(envelope, usage, reported_model.as_deref(), request)
}

pub(super) fn validated_completion(
	envelope: Envelope,
	usage: Value,
	reported_model: Option<&str>,
	request: &Request,
) -> Result<Value> {
	if usage["completion_tokens"]
		.as_u64()
		.is_some_and(|n| n > request.max_tokens as u64)
	{
		return Err(Failure("output_token_limit"));
	}
	if envelope.text.len() > 256 * 1024
		|| envelope.tool_calls.len() > MAX_CALLS
		|| envelope.text.trim().is_empty() && envelope.tool_calls.is_empty()
	{
		return Err(Failure("invalid_response_size"));
	}
	let mut calls = Vec::new();
	for call in envelope.tool_calls {
		if call.arguments.len() > 65536 {
			return Err(Failure("tool_arguments_limit"));
		}
		let tool = request.body["tools"]
			.as_array()
			.and_then(|tools| tools.iter().find(|t| t["function"]["name"] == call.name))
			.ok_or(Failure("unknown_tool"))?;
		let arguments: Value =
			serde_json::from_str(&call.arguments).map_err(|_| Failure("invalid_tool_arguments"))?;
		if !arguments.is_object()
			|| !jsonschema::validator_for(&tool["function"]["parameters"])
				.map_err(|_| Failure("invalid_tool_schema"))?
				.is_valid(&arguments)
		{
			return Err(Failure("invalid_tool_arguments"));
		}
		calls.push(json!({"id":format!("call_{}",uuid::Uuid::new_v4()),"type":"function","function":{"name":call.name,"arguments":call.arguments}}));
	}
	Ok(
		json!({"id":format!("cli-{}",uuid::Uuid::new_v4()),"object":"chat.completion",
		"model":reported_model,"usage":usage,"choices":[{"index":0,
		"finish_reason":if calls.is_empty(){"stop"}else{"tool_calls"},
		"message":{"role":"assistant","content":envelope.text,"tool_calls":calls}}]}),
	)
}

pub(super) fn claude_usage(u: &Value) -> Value {
	let mut usage = json!({});
	// Anthropic input excludes cache; OpenAI-compatible input includes it once.
	if let (Some(input), Some(read), Some(write)) = (
		u["input_tokens"].as_u64(),
		u["cache_read_input_tokens"].as_u64(),
		u["cache_creation_input_tokens"].as_u64(),
	) && let Some(total) = input.checked_add(read).and_then(|n| n.checked_add(write))
	{
		usage["prompt_tokens"] = json!(total);
		usage["prompt_tokens_details"] = json!({"cached_tokens":read,"cache_write_tokens":write});
	}
	if let Some(output) = u["output_tokens"].as_u64() {
		usage["completion_tokens"] = json!(output);
	}
	// CLI USD estimates are not OpenRouter credits. Omit provider cost entirely.
	usage
}
