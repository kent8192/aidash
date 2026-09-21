use crate::{Error, Result, config::secret, registry::ModelConfig};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
	pub name: String,
	pub description: String,
	pub parameters: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
	pub id: String,
	pub name: String,
	pub arguments: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequest {
	pub instructions: String,
	pub context: Value,
	pub tools: Vec<ToolSpec>,
	pub max_output_tokens: u32,
}

impl ModelRequest {
	/// Model-visible payload, shared by transport and context accounting. The
	/// context is encoded as message text, including its JSON escaping.
	pub(crate) fn input_body(&self) -> Value {
		let mut body = json!({"messages":[
			{"role":"system","content":self.instructions},
			{"role":"user","content":self.context.to_string()}]});
		if !self.tools.is_empty() {
			body["tools"] = Value::Array(self.tools.iter().map(|t| json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.parameters}})).collect());
		}
		body
	}

	/// Conservative UTF-8 byte estimate, not a provider tokenizer. Reserve
	/// completion tokens and framing separately, in the same unit at every gate.
	pub(crate) fn estimated_total_tokens(&self) -> usize {
		self.input_body()
			.to_string()
			.len()
			.saturating_add(self.max_output_tokens as usize)
			.saturating_add(1024)
	}
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelResponse {
	pub text: String,
	pub tool_calls: Vec<ToolCall>,
	pub input_tokens: u64,
	pub output_tokens: u64,
	#[serde(default)]
	pub usage_complete: bool,
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
	async fn infer(&self, request: ModelRequest) -> Result<ModelResponse>;
}

pub struct OpenRouterProvider {
	pub client: reqwest::Client,
	pub config: ModelConfig,
}

pub fn provider(client: reqwest::Client, config: ModelConfig) -> Result<Arc<dyn ModelProvider>> {
	match config.provider.as_str() {
		"openrouter" => Ok(Arc::new(OpenRouterProvider { client, config })),
		_ => Err(Error::Invalid("unsupported model provider".into())),
	}
}

#[async_trait]
impl ModelProvider for OpenRouterProvider {
	async fn infer(&self, request: ModelRequest) -> Result<ModelResponse> {
		let mut body = request.input_body();
		body["model"] = json!(self.config.model_id);
		body["max_tokens"] = json!(request.max_output_tokens);
		// Enforce ZDR on every call, including existing registered models. Never
		// retry against non-ZDR endpoints if no eligible provider is available.
		body["provider"] = json!({"zdr": true, "require_parameters": true});
		if let Some(effort) = self.config.reasoning_effort {
			body["reasoning"] = json!({"effort": effort});
		}
		let mut call = self
			.client
			.post(format!(
				"{}/chat/completions",
				self.config.endpoint.trim_end_matches('/')
			))
			.json(&body);
		if let Some(name) = &self.config.credential_env {
			call = call.bearer_auth(secret(name)?);
		}
		let response = call.send().await?;
		if !response.status().is_success() {
			return Err(Error::External(format!(
				"OpenRouter provider returned {}",
				response.status()
			)));
		}
		parse_openai(crate::response::json(response, 1_048_576).await?)
	}
}

pub fn parse_openai(value: Value) -> Result<ModelResponse> {
	let choice = value
		.pointer("/choices/0")
		.ok_or_else(|| Error::External("provider returned no completion choice".into()))?;
	if !matches!(
		choice["finish_reason"].as_str(),
		Some("stop" | "tool_calls")
	) {
		return Err(Error::External(
			"provider output was truncated or refused".into(),
		));
	}
	let message = &choice["message"];
	if !message["refusal"].is_null() {
		return Err(Error::External("provider refused the request".into()));
	}
	let mut result = ModelResponse {
		text: message["content"].as_str().unwrap_or_default().into(),
		usage_complete: value
			.pointer("/usage/prompt_tokens")
			.and_then(Value::as_u64)
			.is_some()
			&& value
				.pointer("/usage/completion_tokens")
				.and_then(Value::as_u64)
				.is_some(),
		input_tokens: value
			.pointer("/usage/prompt_tokens")
			.and_then(Value::as_u64)
			.unwrap_or(0),
		output_tokens: value
			.pointer("/usage/completion_tokens")
			.and_then(Value::as_u64)
			.unwrap_or(0),
		..Default::default()
	};
	if let Some(calls) = message["tool_calls"].as_array() {
		for call in calls {
			result.tool_calls.push(ToolCall {
				id: field(call, "id")?,
				name: field(&call["function"], "name")?,
				arguments: serde_json::from_str(
					call.pointer("/function/arguments")
						.and_then(Value::as_str)
						.ok_or_else(|| Error::External("missing tool arguments".into()))?,
				)
				.map_err(|error| {
					Error::External(format!("invalid provider tool arguments: {error}"))
				})?,
			});
		}
	}
	if (choice["finish_reason"] == "tool_calls") == result.tool_calls.is_empty() {
		return Err(Error::External(
			"provider finish reason does not match tool calls".into(),
		));
	}
	validate_response(&result)?;
	Ok(result)
}

fn field(v: &Value, key: &str) -> Result<String> {
	v[key]
		.as_str()
		.map(str::to_owned)
		.ok_or_else(|| Error::External(format!("provider omitted {key}")))
}
fn validate_response(r: &ModelResponse) -> Result<()> {
	let mut ids = std::collections::HashSet::new();
	if r.text.is_empty() && r.tool_calls.is_empty() {
		return Err(Error::External("empty model response".into()));
	}
	for c in &r.tool_calls {
		if c.id.is_empty() || !ids.insert(&c.id) || !c.arguments.is_object() {
			return Err(Error::External(
				"invalid or duplicate model tool call".into(),
			));
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn parses_openrouter_tool_calls() {
		let result = parse_openai(json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"one","function":{"name":"search","arguments":"{\"q\":\"Rust\"}"}}]}}]})).unwrap();
		assert_eq!(result.tool_calls[0].arguments, json!({"q":"Rust"}));
	}
	#[test]
	fn truncation_cannot_complete_a_task() {
		assert!(
			parse_openai(
				json!({"choices":[{"finish_reason":"length","message":{"content":"partial"}}]})
			)
			.is_err()
		);
	}
}
