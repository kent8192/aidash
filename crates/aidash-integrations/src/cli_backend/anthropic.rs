//! OAuth-token Messages API transport. It never starts a CLI or executes tools.
use super::{
	Failure, Result,
	config::Config,
	protocol::{self, Call, Envelope, Request},
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, time::Duration};

const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";

pub(super) async fn infer(config: &Config, request: &Request) -> Result<Value> {
	call(config, request, ENDPOINT).await
}

pub(super) async fn call(config: &Config, request: &Request, endpoint: &str) -> Result<Value> {
	let body = messages(request)?;
	let client = reqwest::Client::builder()
		.no_proxy()
		.redirect(reqwest::redirect::Policy::none())
		.timeout(Duration::from_secs(config.timeout_seconds))
		.connect_timeout(Duration::from_secs(15))
		.user_agent(concat!("aidash-oauth-backend/", env!("CARGO_PKG_VERSION")))
		.build()
		.map_err(|_| Failure("anthropic_client_failed"))?;
	// This is the explicitly supplied setup-token, not an API key or an
	// extracted CLI credential. Never retry with another credential/header.
	let mut response = client
		.post(endpoint)
		.bearer_auth(config.oauth_token()?)
		.header("anthropic-version", "2023-06-01")
		.header("anthropic-beta", "oauth-2025-04-20")
		.json(&body)
		.send()
		.await
		.map_err(network_failure)?;
	if !response.status().is_success() {
		return Err(Failure(match response.status().as_u16() {
			401 => "anthropic_token_invalid_expired_or_unsupported",
			403 => "anthropic_token_permission_denied",
			404 => "anthropic_model_unavailable",
			429 => "anthropic_rate_or_usage_limit",
			300..=399 => "anthropic_redirect_refused",
			_ => "anthropic_request_failed",
		}));
	}
	let mut bytes = Vec::new();
	while let Some(chunk) = response.chunk().await.map_err(network_failure)? {
		if bytes.len().saturating_add(chunk.len()) > protocol::MAX_OUTPUT {
			return Err(Failure("anthropic_output_limit"));
		}
		bytes.extend_from_slice(&chunk);
	}
	let value = serde_json::from_slice(&bytes).map_err(|_| Failure("invalid_anthropic_json"))?;
	completion(&value, request, &config.model)
}

fn network_failure(error: reqwest::Error) -> Failure {
	Failure(if error.is_timeout() {
		"anthropic_timeout"
	} else {
		"anthropic_network_failed"
	})
}

fn text_blocks(content: &Value) -> Result<Vec<Value>> {
	if content.is_null() {
		return Ok(vec![]);
	}
	if let Some(text) = content.as_str() {
		return Ok(if text.is_empty() {
			vec![]
		} else {
			vec![json!({"type":"text","text":text})]
		});
	}
	content
		.as_array()
		.filter(|parts| {
			parts.iter().all(|p| {
				p["type"] == "text" && p["text"].is_string() && p.get("cache_control").is_none()
			})
		})
		.cloned()
		.ok_or(Failure("unsupported_modality_or_cache"))
}

fn messages(request: &Request) -> Result<Value> {
	let mut system = Vec::new();
	let mut turns: Vec<Value> = Vec::new();
	let mut pending = BTreeSet::new();
	let mut ids = BTreeSet::new();
	for message in request.body["messages"]
		.as_array()
		.ok_or(Failure("invalid_messages"))?
	{
		let role = message["role"]
			.as_str()
			.ok_or(Failure("invalid_message_role"))?;
		let mut content = text_blocks(&message["content"])?;
		if matches!(role, "system" | "developer") {
			if !turns.is_empty() {
				return Err(Failure("system_message_after_conversation"));
			}
			system.extend(content);
			continue;
		}
		let role = if role == "tool" {
			let id = message["tool_call_id"]
				.as_str()
				.ok_or(Failure("invalid_tool_result"))?;
			if !pending.remove(id) {
				return Err(Failure("unmatched_tool_result"));
			}
			content = vec![json!({"type":"tool_result","tool_use_id":id,"content":content})];
			"user"
		} else {
			if !pending.is_empty() {
				return Err(Failure("missing_tool_results"));
			}
			if let Some(calls) = message.get("tool_calls") {
				let calls = calls.as_array().ok_or(Failure("invalid_tool_history"))?;
				if role != "assistant" || calls.len() > protocol::MAX_CALLS {
					return Err(Failure("invalid_tool_history"));
				}
				for call in calls {
					let id = call["id"].as_str().ok_or(Failure("invalid_tool_history"))?;
					let name = call["function"]["name"]
						.as_str()
						.ok_or(Failure("invalid_tool_history"))?;
					let input: Value = serde_json::from_str(
						call["function"]["arguments"]
							.as_str()
							.ok_or(Failure("invalid_tool_history"))?,
					)
					.map_err(|_| Failure("invalid_tool_history"))?;
					if call["type"] != "function"
						|| id.is_empty() || !ids.insert(id.to_owned())
						|| !input.is_object()
					{
						return Err(Failure("invalid_tool_history"));
					}
					pending.insert(id.to_owned());
					content.push(json!({"type":"tool_use","id":id,"name":name,"input":input}));
				}
			}
			role
		};
		if content.is_empty() {
			return Err(Failure("empty_message"));
		}
		if let Some(previous) = turns.last_mut().filter(|turn| turn["role"] == role) {
			previous["content"].as_array_mut().unwrap().extend(content);
		} else {
			turns.push(json!({"role":role,"content":content}));
		}
	}
	if !pending.is_empty() {
		return Err(Failure("missing_tool_results"));
	}
	if turns.is_empty() || turns[0]["role"] != "user" {
		return Err(Failure("user_message_required"));
	}
	let mut body = json!({"model":request.body["model"],"max_tokens":request.max_tokens,"messages":turns,"stream":false});
	if !system.is_empty() {
		body["system"] = json!(system);
	}
	if let Some(tools) = request.body["tools"]
		.as_array()
		.filter(|tools| !tools.is_empty())
	{
		// Only ordinary client function schemas. No server-side web, code, MCP,
		// computer-use or file tools are enabled in this request.
		body["tools"] = tools.iter().map(|tool| {
			let f = &tool["function"];
			json!({"name":f["name"],"description":f["description"].as_str().unwrap_or(""),"input_schema":f["parameters"]})
		}).collect();
		body["tool_choice"] = json!({"type":"auto"});
	}
	Ok(body)
}

fn completion(value: &Value, request: &Request, model: &str) -> Result<Value> {
	if value["type"] != "message" || value["role"] != "assistant" {
		return Err(Failure("invalid_anthropic_message"));
	}
	if value["model"].as_str() != Some(model) {
		return Err(Failure("model_mismatch"));
	}
	if !matches!(value["stop_reason"].as_str(), Some("end_turn" | "tool_use")) {
		return Err(Failure("terminal_response"));
	}
	let mut envelope = Envelope {
		text: String::new(),
		tool_calls: vec![],
	};
	let mut ids = BTreeSet::new();
	for block in value["content"]
		.as_array()
		.ok_or(Failure("invalid_anthropic_content"))?
	{
		match block["type"].as_str() {
			Some("text") => envelope.text.push_str(
				block["text"]
					.as_str()
					.ok_or(Failure("invalid_anthropic_content"))?,
			),
			Some("tool_use") => {
				let id = block["id"]
					.as_str()
					.filter(|id| !id.is_empty())
					.ok_or(Failure("invalid_tool_id"))?;
				if !ids.insert(id) {
					return Err(Failure("duplicate_tool_id"));
				}
				envelope.tool_calls.push(Call {
					name: block["name"]
						.as_str()
						.ok_or(Failure("invalid_tool_name"))?
						.into(),
					arguments: block["input"].to_string(),
				});
			}
			_ => return Err(Failure("unsupported_anthropic_content")),
		}
	}
	if (value["stop_reason"] == "tool_use") == envelope.tool_calls.is_empty() {
		return Err(Failure("invalid_anthropic_stop_reason"));
	}
	protocol::validated_completion(
		envelope,
		protocol::claude_usage(&value["usage"]),
		Some(model),
		request,
	)
}

#[cfg(test)]
mod tests;
