//! A logical MCP tool call survives the wire requests and human waits of MRTR.
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const MAX_INPUTS: usize = 16;
pub const MAX_INTERACTION_BYTES: usize = 256_000;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SamplingTarget {
	pub agent: String,
	pub plugin: super::PluginName,
	pub server: super::ServerName,
	pub digest: String,
	pub origin: String,
}
impl SamplingTarget {
	pub fn allow_target(&self) -> String {
		format!(
			"sampling:{}",
			serde_json::json!([
				self.agent,
				self.plugin,
				self.server,
				self.digest,
				self.origin
			])
		)
	}
	pub fn deny_target(&self) -> String {
		format!(
			"sampling:{}",
			serde_json::json!([self.agent, self.plugin, self.server])
		)
	}
}

/// Individual deny rules survive copy changes; an allow or ask must name the
/// exact copy and origin. Wildcards are represented separately by RuleTarget.
pub fn valid_sampling_rule(target: &str, effect: crate::approval_rules::RuleEffect) -> bool {
	let Some(raw) = target.strip_prefix("sampling:") else {
		return false;
	};
	let Ok(parts) = serde_json::from_str::<Vec<String>>(raw) else {
		return false;
	};
	// Rule comparison uses the canonical target emitted by SamplingTarget.
	// Reject other spellings so an accepted deny cannot silently fail to match.
	if serde_json::to_string(&parts).is_ok_and(|canonical| canonical != raw) {
		return false;
	}
	if parts.len()
		!= if effect == crate::approval_rules::RuleEffect::Deny {
			3
		} else {
			5
		} || parts
		.iter()
		.any(|part| part.is_empty() || part.len() > 2048 || part.chars().any(char::is_control))
		|| super::PluginName::try_from(parts[1].clone()).is_err()
		|| super::ServerName::try_from(parts[2].clone()).is_err()
	{
		return false;
	}
	parts.len() == 3
		|| (super::reference::valid_digest(&parts[3])
			&& (parts[4] == "stdio"
				|| url::Url::parse(&parts[4]).is_ok_and(|url| {
					url.origin().ascii_serialization() == parts[4]
						&& matches!(url.scheme(), "http" | "https")
				})))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InteractionLimits {
	pub human_wait_seconds: u64,
	pub continuation_sends: u32,
	pub sampling_tokens: u32,
}
impl Default for InteractionLimits {
	fn default() -> Self {
		Self {
			human_wait_seconds: 900,
			continuation_sends: 10,
			sampling_tokens: 4096,
		}
	}
}

/// Transport adapters return an intermediate result separately from tool data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", content = "data", rename_all = "snake_case")]
pub enum ToolRound {
	Complete(Value),
	InputRequired(RequiredInput),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequiredInput {
	/// Opaque server data, echoed verbatim; it is never interpreted as authority.
	pub request_state: Option<String>,
	pub requests: BTreeMap<String, InputRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputRequest {
	pub method: String,
	pub params: Value,
}

impl RequiredInput {
	pub fn validate(&self) -> Result<()> {
		if (self.request_state.is_none() && self.requests.is_empty())
			|| self.requests.len() > MAX_INPUTS
			|| serde_json::to_vec(self)?.len() > MAX_INTERACTION_BYTES
			|| self
				.requests
				.keys()
				.any(|id| id.is_empty() || id.len() > 256 || id.chars().any(char::is_control))
		{
			return Err(Error::Invalid("MCP_INPUT_INVALID".into()));
		}
		for request in self.requests.values() {
			match request.method.as_str() {
				"sampling/createMessage" => {
					SamplingRequest::parse(&request.params, u32::MAX)?;
				}
				"elicitation/create" => {
					ElicitationRequest::parse(&request.params)?;
				}
				_ => return Err(Error::Invalid("MCP_INPUT_UNSUPPORTED".into())),
			}
		}
		Ok(())
	}
}

/// Pinned to the logical call at the ToolCallState cursor. A completed wire
/// request is journaled before this checkpoint advances, so a lost worker never
/// resends an uncertain continuation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallCheckpoint {
	pub key: String,
	pub continuation_sends: u32,
	pub wire_pending: bool,
	pub input: Option<RequiredInput>,
	pub responses: BTreeMap<String, Value>,
	pub active: Option<ActiveInput>,
	pub waited_millis: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActiveInput {
	pub id: String,
	pub started_at: DateTime<Utc>,
	pub deadline: DateTime<Utc>,
}

impl CallCheckpoint {
	pub fn new(key: String) -> Self {
		Self {
			key,
			continuation_sends: 0,
			wire_pending: false,
			input: None,
			responses: BTreeMap::new(),
			active: None,
			waited_millis: 0,
		}
	}
	pub fn accept(&mut self, input: RequiredInput) -> Result<()> {
		input.validate()?;
		self.input = Some(input);
		self.wire_pending = false;
		self.responses.clear();
		self.active = None;
		Ok(())
	}
	pub fn next_input(&self) -> Option<(&str, &InputRequest)> {
		self.input
			.as_ref()?
			.requests
			.iter()
			.find(|(id, _)| !self.responses.contains_key(*id))
			.map(|(id, request)| (id.as_str(), request))
	}
	pub fn begin_wait(
		&mut self,
		id: &str,
		now: DateTime<Utc>,
		seconds: u64,
		limits: InteractionLimits,
	) -> Result<DateTime<Utc>> {
		if let Some(active) = &self.active {
			return if active.id == id {
				Ok(active.deadline)
			} else {
				Err(Error::Invalid("MCP_INPUT_OVERLAP".into()))
			};
		}
		let remaining = limits
			.human_wait_seconds
			.saturating_mul(1000)
			.saturating_sub(self.waited_millis);
		let millis = remaining
			.min(seconds.saturating_mul(1000))
			.min(i64::MAX as u64);
		let deadline = now + chrono::Duration::milliseconds(millis as i64);
		self.active = Some(ActiveInput {
			id: id.into(),
			started_at: now,
			deadline,
		});
		Ok(deadline)
	}
	pub fn answer(&mut self, id: &str, value: Value, now: DateTime<Utc>) -> Result<()> {
		if !self
			.input
			.as_ref()
			.is_some_and(|input| input.requests.contains_key(id))
			|| self.responses.contains_key(id)
		{
			return Err(Error::Invalid("MCP_INPUT_STALE".into()));
		}
		if let Some(active) = self.active.take() {
			if active.id != id {
				self.active = Some(active);
				return Err(Error::Invalid("MCP_INPUT_OVERLAP".into()));
			}
			self.waited_millis = self.waited_millis.saturating_add(
				(now.min(active.deadline) - active.started_at)
					.num_milliseconds()
					.max(0) as u64,
			);
		}
		self.responses.insert(id.into(), value);
		Ok(())
	}
	pub fn continue_call(&mut self, limits: InteractionLimits) -> Result<()> {
		if self.next_input().is_some() || self.active.is_some() {
			return Err(Error::Invalid("MCP_INPUT_INCOMPLETE".into()));
		}
		if self.continuation_sends >= limits.continuation_sends {
			return Err(Error::Invalid("MCP_CONTINUATION_LIMIT".into()));
		}
		self.continuation_sends += 1;
		Ok(())
	}
}

/// Only text sampling is accepted; no roots, tools, context inclusion or model
/// preferences can enlarge the Run's authority.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SamplingRequest {
	pub messages: Vec<SamplingMessage>,
	pub system_prompt: String,
	pub max_tokens: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SamplingMessage {
	pub role: String,
	pub text: String,
}
impl SamplingRequest {
	pub fn parse(params: &Value, cap: u32) -> Result<Self> {
		let invalid = || Error::Invalid("MCP_SAMPLING_INVALID".into());
		let input = params
			.get("messages")
			.and_then(Value::as_array)
			.filter(|m| !m.is_empty() && m.len() <= 128)
			.ok_or_else(invalid)?;
		if params.get("tools").is_some() || params.get("toolChoice").is_some() {
			return Err(invalid());
		}
		let mut messages = Vec::new();
		for message in input {
			let role = message["role"]
				.as_str()
				.filter(|r| matches!(*r, "user" | "assistant"))
				.ok_or_else(invalid)?;
			let content = &message["content"];
			let parts = match content {
				Value::Array(parts) => parts.iter().collect(),
				_ => vec![content],
			};
			let mut text = String::new();
			for part in parts {
				if part["type"] != "text" {
					return Err(invalid());
				}
				text.push_str(part["text"].as_str().ok_or_else(invalid)?);
			}
			messages.push(SamplingMessage {
				role: role.into(),
				text,
			});
		}
		let max_tokens = params["maxTokens"]
			.as_u64()
			.filter(|v| *v > 0)
			.ok_or_else(invalid)?
			.min(u32::MAX as u64) as u32;
		let system_prompt = match params.get("systemPrompt") {
			None => String::new(),
			Some(value) => value.as_str().ok_or_else(invalid)?.to_owned(),
		};
		if serde_json::to_vec(params)?.len() > MAX_INTERACTION_BYTES
			|| cap == 0
			|| system_prompt.contains('\0')
			|| messages.iter().any(|message| message.text.contains('\0'))
		{
			return Err(invalid());
		}
		Ok(Self {
			messages,
			system_prompt,
			max_tokens: max_tokens.min(cap),
		})
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ElicitationRequest {
	pub message: String,
	pub schema: Value,
}
impl ElicitationRequest {
	pub fn parse(params: &Value) -> Result<Self> {
		let invalid = || Error::Invalid("MCP_ELICITATION_INVALID".into());
		if params.get("mode").is_some_and(|mode| mode != "form") || params.get("url").is_some() {
			return Err(Error::Invalid("MCP_ELICITATION_URL_UNSUPPORTED".into()));
		}
		let message = params["message"].as_str().ok_or_else(invalid)?.to_owned();
		let schema = &params["requestedSchema"];
		if schema.as_object().is_none_or(|fields| {
			fields.keys().any(|key| {
				![
					"type",
					"properties",
					"required",
					"title",
					"description",
					"additionalProperties",
				]
				.contains(&key.as_str())
			})
		}) {
			return Err(invalid());
		}
		let properties = schema["properties"]
			.as_object()
			.filter(|p| p.len() <= 32)
			.ok_or_else(invalid)?;
		if schema
			.get("additionalProperties")
			.is_some_and(|value| !value.is_boolean())
			|| ["title", "description"]
				.iter()
				.any(|key| schema.get(key).is_some_and(|value| !value.is_string()))
		{
			return Err(invalid());
		}
		if schema["type"] != "object" || serde_json::to_vec(params)?.len() > MAX_INTERACTION_BYTES {
			return Err(invalid());
		}
		for property in properties.values() {
			if property.as_object().is_none_or(|fields| {
				fields.keys().any(|key| {
					![
						"type",
						"title",
						"description",
						"default",
						"enum",
						"const",
						"minLength",
						"maxLength",
						"minimum",
						"maximum",
						"exclusiveMinimum",
						"exclusiveMaximum",
						"multipleOf",
						"format",
						"pattern",
					]
					.contains(&key.as_str())
				})
			}) {
				return Err(invalid());
			}

			if !matches!(
				property["type"].as_str(),
				Some("string" | "number" | "integer" | "boolean")
			) || property.get("$ref").is_some()
				|| property.get("properties").is_some()
			{
				return Err(invalid());
			}
			let primitive = |value: &Value| match property["type"].as_str() {
				Some("string") => value.is_string(),
				Some("boolean") => value.is_boolean(),
				Some("integer") => value.is_i64() || value.is_u64(),
				Some("number") => value.is_number(),
				_ => false,
			};
			if ["title", "description", "format", "pattern"]
				.iter()
				.any(|key| property.get(key).is_some_and(|value| !value.is_string()))
				|| ["default", "const"]
					.iter()
					.any(|key| property.get(key).is_some_and(|value| !primitive(value)))
				|| property.get("enum").is_some_and(|values| {
					!values.as_array().is_some_and(|values| {
						!values.is_empty() && values.len() <= 256 && values.iter().all(primitive)
					})
				}) {
				return Err(invalid());
			}
		}
		if schema.get("required").is_some_and(|required| {
			!required.as_array().is_some_and(|names| {
				names.iter().all(|name| {
					name.as_str()
						.is_some_and(|name| properties.contains_key(name))
				})
			})
		}) {
			return Err(invalid());
		}
		Ok(Self {
			message,
			schema: schema.clone(),
		})
	}
	pub fn response(&self, answer: &Value) -> Result<Value> {
		let action = answer["action"].as_str().unwrap_or("decline");
		if matches!(action, "decline" | "cancel") {
			return Ok(serde_json::json!({"action": action}));
		}
		if action != "accept" {
			return Err(Error::Invalid("MCP_ELICITATION_ANSWER_INVALID".into()));
		}
		let content = answer
			.get("content")
			.ok_or_else(|| Error::Invalid("MCP_ELICITATION_ANSWER_INVALID".into()))?;
		let object = content
			.as_object()
			.ok_or_else(|| Error::Invalid("MCP_ELICITATION_ANSWER_INVALID".into()))?;
		let props = self.schema["properties"]
			.as_object()
			.expect("validated form properties");
		if object.keys().any(|key| !props.contains_key(key)) {
			return Err(Error::Invalid("MCP_ELICITATION_ANSWER_INVALID".into()));
		}
		// The boundary's JSON Schema validator checks formats, ranges and enums.
		Ok(serde_json::json!({"action": "accept", "content": content}))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn sampling_rules_refuse_spellings_that_would_not_match_a_server_target() {
		use crate::approval_rules::RuleEffect;
		let target = SamplingTarget {
			agent: "research".into(),
			plugin: "docs".to_owned().try_into().unwrap(),
			server: "writer".to_owned().try_into().unwrap(),
			digest: format!("sha256:{}", "a".repeat(64)),
			origin: "https://example.com".into(),
		};
		assert!(valid_sampling_rule(
			&target.allow_target(),
			RuleEffect::Allow
		));
		assert!(valid_sampling_rule(&target.deny_target(), RuleEffect::Deny));
		assert!(!valid_sampling_rule(
			"sampling:[\"research\", \"docs\", \"writer\"]",
			RuleEffect::Deny
		));
		assert!(!valid_sampling_rule(
			&target.allow_target(),
			RuleEffect::Deny
		));
		assert!(!valid_sampling_rule(
			&target.deny_target(),
			RuleEffect::Allow
		));
	}

	#[test]
	fn one_call_keeps_its_wait_allowance_across_input_batches_and_state_only_rounds() {
		let now = DateTime::parse_from_rfc3339("2026-10-23T00:00:00Z")
			.unwrap()
			.with_timezone(&Utc);
		let limits = InteractionLimits {
			human_wait_seconds: 60,
			continuation_sends: 2,
			..Default::default()
		};
		let form = InputRequest {
			method: "elicitation/create".into(),
			params: json!({"message":"Choose","requestedSchema":{"type":"object","properties":{"ok":{"type":"boolean"}}}}),
		};
		let mut call = CallCheckpoint::new("call".into());
		call.accept(RequiredInput {
			request_state: Some("opaque:first".into()),
			requests: [("one".into(), form.clone())].into(),
		})
		.unwrap();
		assert_eq!(
			call.begin_wait("one", now, 300, limits).unwrap(),
			now + chrono::Duration::seconds(60)
		);
		call.answer(
			"one",
			json!({"action":"decline"}),
			now + chrono::Duration::seconds(45),
		)
		.unwrap();
		call.continue_call(limits).unwrap();
		call.accept(RequiredInput {
			request_state: Some("opaque:second".into()),
			requests: [("two".into(), form)].into(),
		})
		.unwrap();
		assert_eq!(
			call.begin_wait("two", now + chrono::Duration::seconds(45), 300, limits)
				.unwrap(),
			now + chrono::Duration::seconds(60)
		);
		assert!(call.begin_wait("overlapping", now, 300, limits).is_err());
		call.answer(
			"two",
			json!({"action":"cancel"}),
			now + chrono::Duration::seconds(60),
		)
		.unwrap();
		call.continue_call(limits).unwrap();
		call.accept(RequiredInput {
			request_state: Some("opaque:no-input".into()),
			requests: BTreeMap::new(),
		})
		.unwrap();
		assert!(
			call.continue_call(limits)
				.unwrap_err()
				.to_string()
				.contains("MCP_CONTINUATION_LIMIT")
		);
		assert_eq!(call.waited_millis, 60_000);
	}

	#[test]
	fn a_sampling_request_sends_only_server_messages_and_the_capped_output_without_cache_or_tools()
	{
		let sample = SamplingRequest::parse(&json!({"messages":[{"role":"user","content":{"type":"text","text":"server question"}},{"role":"assistant","content":{"type":"text","text":"prior server answer"}}],"systemPrompt":"server instructions","includeContext":"allServers","modelPreferences":{"hints":[{"name":"different-model"}]},"maxTokens":99999}), 32).unwrap();
		let request = crate::provider::ModelRequest {
			instructions: sample.system_prompt,
			context: crate::provider::ModelContext::Sampling(crate::provider::SamplingContext {
				sampling_messages: sample.messages,
			}),
			max_output_tokens: sample.max_tokens,
			..Default::default()
		};
		assert_eq!(request.max_output_tokens, 32);
		assert_eq!(
			request.input_body(),
			json!({"messages":[{"role":"system","content":"server instructions"},{"role":"user","content":"server question"},{"role":"assistant","content":"prior server answer"}]})
		);
		assert!(request.cache_identity().is_none());
		assert!(request.estimated_total_tokens() > 32);
	}

	#[test]
	fn elicitation_rejects_url_mode_nested_values_and_schema_references_before_loading_a_form() {
		for params in [
			json!({"mode":"url","message":"authenticate","url":"https://example.com"}),
			json!({"message":"nested","requestedSchema":{"type":"object","properties":{"nested":{"type":"object","properties":{}}}}}),
			json!({"message":"reference","requestedSchema":{"type":"object","properties":{},"$ref":"http://127.0.0.1/schema"}}),
			json!({"message":"reference","requestedSchema":{"type":"object","properties":{"name":{"type":"string","$dynamicRef":"http://127.0.0.1/schema"}}}}),
			json!({"message":"reference","requestedSchema":{"type":"object","properties":{},"additionalProperties":{"$ref":"http://127.0.0.1/schema"}}}),
			json!({"message":"invalid label","requestedSchema":{"type":"object","properties":{"name":{"type":"string","title":{}}}}}),
			json!({"message":"invalid enum","requestedSchema":{"type":"object","properties":{"name":{"type":"string","enum":{}}}}}),
		] {
			assert!(ElicitationRequest::parse(&params).is_err(), "{params}");
		}
	}
}
