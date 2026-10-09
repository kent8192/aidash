//! OpenRouter inference transport implementing the application port.
use crate::{Error, Result};
use aidash_application::ports::{Credentials, ModelProvider};
use aidash_domain::{
	model::ModelConfig,
	provider::{ContentPart, ModelRequest, ModelResponse, ToolCall},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;

pub mod cache_salt;
pub use cache_salt::{CacheSaltKey, CacheSaltKeys};

pub struct OpenRouterProvider {
	pub client: reqwest::Client,
	pub config: ModelConfig,
	pub credentials: Arc<dyn Credentials>,
	/// This node's Cache Salt Keys; `None` rejects every salted request.
	pub cache_salt: Option<CacheSaltKeys>,
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
			let credential = self.credentials.resolve(name)?;
			endpoints = endpoints.bearer_auth(&credential);
			zdr = zdr.bearer_auth(&credential);
		}
		let (endpoints, zdr) =
			tokio::try_join!(endpoints.send(), zdr.send()).map_err(crate::http_error)?;
		let endpoints = crate::response::json::<Value>(
			endpoints.error_for_status().map_err(crate::http_error)?,
			2 * 1024 * 1024,
		)
		.await?;
		let zdr = crate::response::json::<Value>(
			zdr.error_for_status().map_err(crate::http_error)?,
			8 * 1024 * 1024,
		)
		.await?;
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
			return Err(Error::MediaRouteUnavailable(self.config.model_id.clone()));
		}
		Ok(eligible)
	}
}

pub fn provider(
	client: reqwest::Client,
	config: ModelConfig,
	credentials: Arc<dyn Credentials>,
	cache_salt: Option<CacheSaltKeys>,
) -> Result<Arc<dyn ModelProvider>> {
	match config.provider.as_str() {
		"openrouter" => {
			config.request_timeout()?;
			Ok(Arc::new(OpenRouterProvider {
				client,
				config,
				credentials,
				cache_salt,
			}))
		}
		_ => Err(Error::Invalid("unsupported model provider".into())),
	}
}

#[async_trait]
impl ModelProvider for OpenRouterProvider {
	async fn infer(&self, request: ModelRequest) -> Result<ModelResponse> {
		let deadline = self.config.request_timeout()?;
		tokio::time::timeout(deadline, async {
			request.validate()?;
			// Derive the salt before any provider traffic: a node without the
			// requested key version sends nothing rather than an unsalted request.
			let salt = request
				.cache_scope
				.as_ref()
				.map(|scope| {
					self.cache_salt
						.as_ref()
						.ok_or_else(|| {
							Error::Invalid(format!(
								"Cache Salt Key v{} is not configured on this node",
								scope.key_version
							))
						})?
						.line(scope)
				})
				.transpose()?;
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
			if let Some(salt) = salt {
				let system = &mut body["messages"][0]["content"];
				let salted = format!("{salt}{}", system.as_str().unwrap_or_default());
				*system = Value::String(salted);
			}
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
			.timeout(deadline)
			.json(&body);
			if let Some(name) = &self.config.credential_env {
				call = call.bearer_auth(self.credentials.resolve(name)?);
			}
			let started = std::time::Instant::now();
			let response = call.send().await.map_err(crate::http_error)?;
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
		})
		.await
		.map_err(|_| Error::External("model inference timed out".into()))?
	}
}

fn safe_upstream_reason(detail: &str) -> String {
	let normalized = detail.to_ascii_lowercase();
	if normalized.contains("audio")
		&& (normalized.contains("exceed")
			|| normalized.contains("too long")
			|| normalized.contains("duration limit"))
	{
		return "Audio exceeds the provider limit".into();
	}
	"upstream rejected the request".into()
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
mod tests;
