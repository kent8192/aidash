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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn infer(&self, request: ModelRequest) -> Result<ModelResponse>;
}

pub struct OpenAiProvider {
    pub client: reqwest::Client,
    pub config: ModelConfig,
}
pub struct AnthropicProvider {
    pub client: reqwest::Client,
    pub config: ModelConfig,
}

pub fn provider(client: reqwest::Client, config: ModelConfig) -> Result<Arc<dyn ModelProvider>> {
    match config.provider.as_str() {
        "openai" | "openrouter" => Ok(Arc::new(OpenAiProvider { client, config })),
        "anthropic" => Ok(Arc::new(AnthropicProvider { client, config })),
        _ => Err(Error::Invalid("unsupported model provider".into())),
    }
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    async fn infer(&self, request: ModelRequest) -> Result<ModelResponse> {
        let mut body = json!({"model":self.config.model_id,"messages":[
            {"role":"system","content":request.instructions},
            {"role":"user","content":request.context.to_string()}]});
        let token_limit = if self.config.provider == "openrouter" {
            "max_tokens"
        } else {
            "max_completion_tokens"
        };
        body[token_limit] = json!(request.max_output_tokens);
        if !request.tools.is_empty() {
            body["tools"] = Value::Array(request.tools.iter().map(|t| json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.parameters}})).collect());
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
                "OpenAI-compatible provider returned {}",
                response.status()
            )));
        }
        parse_openai(response.json().await?)
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
                )?,
            });
        }
    }
    validate_response(&result)?;
    Ok(result)
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    async fn infer(&self, request: ModelRequest) -> Result<ModelResponse> {
        let mut body = json!({"model":self.config.model_id,"system":request.instructions,
            "messages":[{"role":"user","content":request.context.to_string()}],"max_tokens":request.max_output_tokens});
        if !request.tools.is_empty() {
            body["tools"] = Value::Array(request.tools.iter().map(|t| json!({"name":t.name,"description":t.description,"input_schema":t.parameters})).collect());
        }
        let mut call = self
            .client
            .post(format!(
                "{}/messages",
                self.config.endpoint.trim_end_matches('/')
            ))
            .header("anthropic-version", "2023-06-01")
            .json(&body);
        if let Some(name) = &self.config.credential_env {
            call = call.header("x-api-key", secret(name)?);
        }
        let response = call.send().await?;
        if !response.status().is_success() {
            return Err(Error::External(format!(
                "Anthropic provider returned {}",
                response.status()
            )));
        }
        parse_anthropic(response.json().await?)
    }
}

pub fn parse_anthropic(value: Value) -> Result<ModelResponse> {
    if !matches!(value["stop_reason"].as_str(), Some("end_turn" | "tool_use")) {
        return Err(Error::External(
            "Anthropic output was truncated or refused".into(),
        ));
    }
    let mut result = ModelResponse {
        input_tokens: value
            .pointer("/usage/input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: value
            .pointer("/usage/output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        ..Default::default()
    };
    for block in value["content"]
        .as_array()
        .ok_or_else(|| Error::External("missing Anthropic content".into()))?
    {
        match block["type"].as_str() {
            Some("text") => result.text.push_str(&field(block, "text")?),
            Some("tool_use") => result.tool_calls.push(ToolCall {
                id: field(block, "id")?,
                name: field(block, "name")?,
                arguments: block["input"].clone(),
            }),
            _ => {}
        }
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
    fn adapters_share_one_response_contract() {
        let a = parse_openai(json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"one","function":{"name":"search","arguments":"{\"q\":\"Rust\"}"}}]}}]})).unwrap();
        let b = parse_anthropic(json!({"stop_reason":"tool_use","content":[{"type":"tool_use","id":"one","name":"search","input":{"q":"Rust"}}]})).unwrap();
        assert_eq!(
            serde_json::to_value(a).unwrap(),
            serde_json::to_value(b).unwrap()
        );
    }
    #[test]
    fn truncation_cannot_complete_a_task() {
        assert!(
            parse_openai(
                json!({"choices":[{"finish_reason":"length","message":{"content":"partial"}}]})
            )
            .is_err()
        );
        assert!(parse_anthropic(json!({"stop_reason":"max_tokens","content":[]})).is_err());
    }
}
