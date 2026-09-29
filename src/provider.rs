use crate::{Error, Result, config::secret, registry::ModelConfig};
use async_trait::async_trait;
use base64::Engine;
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
	/// Resolved, authorized input for this inference only. Never persist bytes
	/// in the durable context or serialize them with the request metadata.
	#[serde(skip)]
	pub content_parts: Vec<ContentPart>,
}

#[derive(Debug, Clone)]
pub enum ContentPart {
	Text(String),
	Image { media_type: String, bytes: Vec<u8> },
	Audio { format: String, bytes: Vec<u8> },
}

impl ContentPart {
	pub fn format_for_media_type(media_type: &str) -> Result<&'static str> {
		match media_type {
			"image/png" => Ok("image/png"),
			"image/jpeg" => Ok("image/jpeg"),
			"image/gif" => Ok("image/gif"),
			"image/webp" => Ok("image/webp"),
			"audio/wav" | "audio/x-wav" => Ok("wav"),
			"audio/mpeg" => Ok("mp3"),
			"audio/mp4" | "audio/x-m4a" => Ok("m4a"),
			"audio/aac" => Ok("aac"),
			"audio/ogg" => Ok("ogg"),
			"audio/webm" => Ok("webm"),
			"audio/flac" | "audio/x-flac" => Ok("flac"),
			_ => Err(Error::Invalid(format!(
				"unsupported model media type: {media_type}"
			))),
		}
	}

	pub fn modality_for_media_type(media_type: &str) -> Result<&'static str> {
		match media_type {
			"image/png" | "image/jpeg" | "image/gif" | "image/webp" => Ok("image"),
			"audio/wav" | "audio/x-wav" | "audio/mpeg" | "audio/mp4" | "audio/x-m4a"
			| "audio/aac" | "audio/ogg" | "audio/webm" | "audio/flac" | "audio/x-flac" => Ok("audio"),
			_ => Err(Error::Invalid(format!(
				"unsupported model media type: {media_type}"
			))),
		}
	}

	pub fn from_media(media_type: &str, bytes: Vec<u8>) -> Result<Self> {
		let part = match Self::modality_for_media_type(media_type)? {
			"image" => Self::Image {
				media_type: media_type.into(),
				bytes,
			},
			_ => Self::Audio {
				format: Self::format_for_media_type(media_type)?.into(),
				bytes,
			},
		};
		part.validate()?;
		Ok(part)
	}

	fn openrouter(&self) -> Value {
		match self {
			Self::Text(text) => json!({"type":"text","text":text}),
			Self::Image { media_type, bytes } => {
				json!({"type":"image_url","image_url":{"url":format!("data:{media_type};base64,{}",base64::engine::general_purpose::STANDARD.encode(bytes))}})
			}
			Self::Audio { format, bytes } => {
				json!({"type":"input_audio","input_audio":{"data":base64::engine::general_purpose::STANDARD.encode(bytes),"format":format}})
			}
		}
	}

	fn validate(&self) -> Result<()> {
		match self {
			Self::Text(_) => Ok(()),
			Self::Image { media_type, bytes } => {
				let valid = match media_type.as_str() {
					"image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
					"image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
					"image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
					"image/webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
					_ => false,
				};
				if !valid {
					return Err(Error::Invalid(
						"image MIME does not match a supported file signature".into(),
					));
				}
				Ok(())
			}
			Self::Audio { format, bytes } => {
				let valid = match format.as_str() {
					"wav" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE"),
					"mp3" => {
						bytes.starts_with(b"ID3")
							|| bytes.starts_with(b"\xff\xfb")
							|| bytes.starts_with(b"\xff\xf3")
							|| bytes.starts_with(b"\xff\xf2")
					}
					"m4a" => bytes.get(4..8) == Some(b"ftyp"),
					"aac" => bytes.starts_with(b"\xff\xf1") || bytes.starts_with(b"\xff\xf9"),
					"ogg" => bytes.starts_with(b"OggS"),
					"webm" => bytes.starts_with(b"\x1a\x45\xdf\xa3"),
					"flac" => bytes.starts_with(b"fLaC"),
					_ => false,
				};
				if !valid {
					return Err(Error::Invalid(
						"audio format does not match a supported file signature".into(),
					));
				}
				Ok(())
			}
		}
	}
}

impl ModelRequest {
	/// Model-visible payload, shared by transport and context accounting. The
	/// context is encoded as message text, including its JSON escaping.
	pub(crate) fn input_body(&self) -> Value {
		let content = if self.content_parts.is_empty() {
			Value::String(self.context.to_string())
		} else {
			Value::Array(
				std::iter::once(json!({"type":"text","text":self.context.to_string()}))
					.chain(self.content_parts.iter().map(ContentPart::openrouter))
					.collect(),
			)
		};
		let mut body = json!({"messages":[
			{"role":"system","content":self.instructions},
			{"role":"user","content":content}]});
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

	fn validate(&self) -> Result<()> {
		let mut media_count = 0;
		let mut media_bytes = 0_usize;
		for part in &self.content_parts {
			part.validate()?;
			if let ContentPart::Image { bytes, .. } | ContentPart::Audio { bytes, .. } = part {
				media_count += 1;
				media_bytes = media_bytes.saturating_add(bytes.len());
			}
		}
		if media_count > 8 || media_bytes > 8 * 1024 * 1024 {
			return Err(Error::Invalid(
				"model media input exceeds count or byte limit".into(),
			));
		}
		Ok(())
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

impl OpenRouterProvider {
	async fn verified_media_routes(&self, request: &ModelRequest) -> Result<Vec<String>> {
		let formats: Vec<&str> = request
			.content_parts
			.iter()
			.filter_map(|part| match part {
				ContentPart::Image { media_type, .. } => Some(media_type.as_str()),
				ContentPart::Audio { format, .. } => Some(format.as_str()),
				ContentPart::Text(_) => None,
			})
			.collect();
		if formats.is_empty() {
			return Ok(Vec::new());
		}
		if self.config.media_routes.is_empty() {
			return Err(Error::Invalid(format!(
				"model {} has no verified media route",
				self.config.model_id
			)));
		}
		let mut url = reqwest::Url::parse(&self.config.endpoint)
			.map_err(|_| Error::Invalid("invalid OpenRouter endpoint".into()))?;
		{
			let mut segments = url
				.path_segments_mut()
				.map_err(|_| Error::Invalid("invalid OpenRouter endpoint".into()))?;
			segments.pop_if_empty().push("models");
			for segment in self.config.model_id.split('/') {
				if segment.is_empty() || segment == "." || segment == ".." {
					return Err(Error::Invalid("invalid OpenRouter model ID".into()));
				}
				segments.push(segment);
			}
			segments.push("endpoints");
		}
		let mut endpoints = self
			.client
			.get(url)
			.timeout(std::time::Duration::from_secs(15));
		let mut zdr = self
			.client
			.get(format!(
				"{}/endpoints/zdr",
				self.config.endpoint.trim_end_matches('/')
			))
			.timeout(std::time::Duration::from_secs(15));
		if let Some(name) = &self.config.credential_env {
			let credential = secret(name)?;
			endpoints = endpoints.bearer_auth(&credential);
			zdr = zdr.bearer_auth(&credential);
		}
		let (endpoints, zdr) = tokio::try_join!(endpoints.send(), zdr.send())?;
		let endpoints =
			crate::response::json::<Value>(endpoints.error_for_status()?, 2 * 1024 * 1024).await?;
		let zdr = crate::response::json::<Value>(zdr.error_for_status()?, 8 * 1024 * 1024).await?;
		let modalities = endpoints
			.pointer("/data/architecture/input_modalities")
			.and_then(Value::as_array)
			.ok_or_else(|| Error::External("OpenRouter model modalities unavailable".into()))?;
		for part in &request.content_parts {
			let modality = match part {
				ContentPart::Image { .. } => "image",
				ContentPart::Audio { .. } => "audio",
				ContentPart::Text(_) => continue,
			};
			if !modalities.iter().any(|value| value == modality) {
				return Err(Error::Invalid(format!(
					"OpenRouter model {} no longer supports {modality} input",
					self.config.model_id
				)));
			}
		}
		let endpoints = endpoints
			.pointer("/data/endpoints")
			.and_then(Value::as_array)
			.ok_or_else(|| Error::External("OpenRouter endpoint list unavailable".into()))?;
		let zdr = zdr
			.get("data")
			.and_then(Value::as_array)
			.ok_or_else(|| Error::External("OpenRouter ZDR endpoint list unavailable".into()))?;
		let now = chrono::Utc::now();
		let mut eligible = Vec::new();
		for evidence in &self.config.media_routes {
			if evidence.verified_at > now
				|| evidence.expires_at <= now
				|| !formats
					.iter()
					.all(|format| evidence.formats.iter().any(|verified| verified == format))
			{
				continue;
			}
			// A base slug also matches variants. An exact route can only be
			// allowlisted when it cannot select an unverified sibling variant.
			if !evidence.tag.contains('/')
				&& endpoints.iter().any(|endpoint| {
					endpoint["tag"]
						.as_str()
						.is_some_and(|tag| tag.starts_with(&format!("{}/", evidence.tag)))
				}) {
				continue;
			}
			let present = endpoints.iter().any(|endpoint| {
				endpoint["tag"] == evidence.tag
					&& endpoint["context_length"]
						.as_u64()
						.is_none_or(|limit| request.estimated_total_tokens() as u64 <= limit)
			});
			let private = zdr.iter().any(|endpoint| {
				endpoint["model_id"] == self.config.model_id && endpoint["tag"] == evidence.tag
			});
			if present && private {
				eligible.push(evidence.tag.clone());
			}
		}
		if eligible.is_empty() {
			return Err(Error::Invalid(format!(
				"no verified ZDR route supports every media format for model {}",
				self.config.model_id
			)));
		}
		Ok(eligible)
	}
}

pub fn provider(client: reqwest::Client, config: ModelConfig) -> Result<Arc<dyn ModelProvider>> {
	match config.provider.as_str() {
		"openrouter" => {
			config.request_timeout()?;
			Ok(Arc::new(OpenRouterProvider { client, config }))
		}
		_ => Err(Error::Invalid("unsupported model provider".into())),
	}
}

#[async_trait]
impl ModelProvider for OpenRouterProvider {
	async fn infer(&self, request: ModelRequest) -> Result<ModelResponse> {
		request.validate()?;
		for modality in request.content_parts.iter().filter_map(|part| match part {
			ContentPart::Image { .. } => Some("image"),
			ContentPart::Audio { .. } => Some("audio"),
			ContentPart::Text(_) => None,
		}) {
			if !self
				.config
				.modalities
				.iter()
				.any(|available| available == modality)
			{
				return Err(Error::Invalid(format!(
					"model {} does not support {modality} input",
					self.config.model_id
				)));
			}
		}
		let media_routes = self.verified_media_routes(&request).await?;
		let mut body = request.input_body();
		body["model"] = json!(self.config.model_id);
		body["max_tokens"] = json!(request.max_output_tokens);
		// Enforce ZDR on every call, including existing registered models. Never
		// retry against non-ZDR endpoints if no eligible provider is available.
		body["provider"] = if media_routes.is_empty() {
			json!({"zdr": true, "require_parameters": true})
		} else {
			json!({"zdr": true, "require_parameters": true, "only": media_routes, "allow_fallbacks": true})
		};
		if let Some(effort) = self.config.reasoning_effort {
			body["reasoning"] = json!({"effort": effort});
		}
		let mut call = self
			.client
			.post(format!(
				"{}/chat/completions",
				self.config.endpoint.trim_end_matches('/')
			))
			// Override only inference, including response-body reads. Other HTTP
			// traffic retains the shared client's timeout and connection policy.
			.timeout(self.config.request_timeout()?)
			.json(&body);
		if let Some(name) = &self.config.credential_env {
			call = call.bearer_auth(secret(name)?);
		}
		let started = std::time::Instant::now();
		let response = call.send().await?;
		metrics::histogram!("aidash_model_response_headers_seconds")
			.record(started.elapsed().as_secs_f64());
		if !response.status().is_success() {
			let status = response.status();
			let detail = crate::response::json::<Value>(response, 16_384)
				.await
				.ok()
				.and_then(|body| {
					body.pointer("/error/message")
						.and_then(Value::as_str)
						.map(str::to_owned)
				})
				.unwrap_or_else(|| {
					"upstream rejected the request without a readable reason".into()
				});
			return Err(Error::ProviderRejected {
				status: status.as_u16(),
				reason: safe_upstream_reason(&detail),
			});
		}
		let result = parse_openai(crate::response::json(response, 1_048_576).await?)?;
		metrics::counter!("aidash_model_tokens_total", "direction" => "input")
			.increment(result.input_tokens);
		metrics::counter!("aidash_model_tokens_total", "direction" => "output")
			.increment(result.output_tokens);
		Ok(result)
	}
}

fn safe_upstream_reason(detail: &str) -> String {
	let detail = detail.split_whitespace().collect::<Vec<_>>().join(" ");
	if detail.contains("base64,") || detail.contains("Bearer ") || detail.contains("sk-") {
		return "upstream rejected the media input".into();
	}
	detail.chars().take(512).collect()
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
	let content = message["content"].as_str().unwrap_or_default();
	let mut result = ModelResponse {
		text: if content.trim().is_empty() {
			String::new()
		} else {
			content.to_owned()
		},
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
			parse_openai(
				json!({"choices":[{"finish_reason":"stop","message":{"content":" \n\t "}}]})
			)
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
}
