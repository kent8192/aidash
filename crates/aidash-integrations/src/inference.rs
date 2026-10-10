//! OpenRouter inference transport implementing the application port.
use crate::{Error, Result};
use aidash_application::ports::ModelProvider;
use aidash_application::provider_access::{Context, Inference, Operation, ProviderAccess, Source};
use aidash_domain::{
	context::recovery::Failure,
	model::ModelConfig,
	provider::{ContentPart, ModelRequest, ModelResponse, ToolCall},
};
use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use std::sync::Arc;

pub mod cache_salt;
pub use cache_salt::{CacheSaltKey, CacheSaltKeys};

pub struct OpenRouterProvider {
	pub client: reqwest::Client,
	pub config: ModelConfig,
	pub access: Arc<dyn ProviderAccess>,
	pub context: Context,
	/// This node's Cache Salt Keys; `None` rejects every salted request.
	pub cache_salt: Option<CacheSaltKeys>,
}

impl OpenRouterProvider {
	fn call_context(&self, operation: Operation, max_output_tokens: u32) -> Context {
		let mut context = self.context.clone();
		context.inference = Some(Inference {
			model: self.config.model_id.clone(),
			operations: vec![operation],
			max_output_tokens,
		});
		context
	}
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
		let access = self
			.access
			.resolve(
				&self.call_context(Operation::Discovery, self.config.output_token_limit()),
				&self.config.endpoint,
				&Source::configured(
					&self.config.credential_env,
					&self.config.provider_credential,
				),
			)
			.await?;
		let mut url = reqwest::Url::parse(&access.endpoint)
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
				access.endpoint.trim_end_matches('/')
			))
			.timeout(std::time::Duration::from_secs(15));
		if !access.bearer.expose_secret().is_empty() {
			endpoints = endpoints.bearer_auth(access.bearer.expose_secret());
			zdr = zdr.bearer_auth(access.bearer.expose_secret());
		}
		let (endpoints, zdr) =
			tokio::try_join!(endpoints.send(), zdr.send()).map_err(crate::http_error)?;
		if !endpoints.status().is_success() {
			return Err(crate::response::provider_rejection(
				endpoints,
				self.config.provider_credential.is_some(),
			)
			.await);
		}
		if !zdr.status().is_success() {
			return Err(crate::response::provider_rejection(
				zdr,
				self.config.provider_credential.is_some(),
			)
			.await);
		}
		let endpoints = crate::response::json::<Value>(endpoints, 2 * 1024 * 1024).await?;
		let zdr = crate::response::json::<Value>(zdr, 8 * 1024 * 1024).await?;
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

/// A provider without Cache Salt Keys: every salted request is rejected
/// before any provider traffic.
pub fn provider(
	client: reqwest::Client,
	config: ModelConfig,
	access: Arc<dyn ProviderAccess>,
	context: Context,
) -> Result<Arc<dyn ModelProvider>> {
	salted_provider(client, config, access, context, None)
}

/// A provider that salts requests carrying a Cache Scope with this node's keys.
pub fn salted_provider(
	client: reqwest::Client,
	config: ModelConfig,
	access: Arc<dyn ProviderAccess>,
	context: Context,
	cache_salt: Option<CacheSaltKeys>,
) -> Result<Arc<dyn ModelProvider>> {
	aidash_domain::provider_credentials::validate_source(
		&config.endpoint,
		&config.provider,
		config.credential_env.as_deref(),
		config.provider_credential.as_deref(),
	)?;
	match config.provider.as_str() {
		"openrouter" => {
			config.request_timeout()?;
			Ok(Arc::new(OpenRouterProvider {
				client,
				config,
				access,
				context,
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
			if self.config.provider_credential.is_some()
				&& request.max_output_tokens > self.config.output_token_limit()
			{
				return Err(Error::Invalid(
					"BYOK inference exceeds the configured output limit".into(),
				));
			}
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
			let body = request_body(&self.config, &request, media_routes, salt.as_deref());
			let access = self
				.access
				.resolve(
					&self.call_context(Operation::Chat, request.max_output_tokens),
					&self.config.endpoint,
					&Source::configured(
						&self.config.credential_env,
						&self.config.provider_credential,
					),
				)
				.await?;
			let mut call = self
			.client
			.post(format!(
				"{}/chat/completions",
				access.endpoint.trim_end_matches('/')
			))
			// Override only inference, including response-body reads. Other HTTP
			// traffic retains the shared client's timeout and connection policy.
			.timeout(deadline)
			.json(&body);
			if !access.bearer.expose_secret().is_empty() {
				call = call.bearer_auth(access.bearer.expose_secret());
			}
			let started = std::time::Instant::now();
			let response = call.send().await.map_err(crate::http_error)?;
			metrics::histogram!("aidash_model_response_headers_seconds")
				.record(started.elapsed().as_secs_f64());
			if !response.status().is_success() {
				let status = response.status().as_u16();
				let body = crate::response::json::<Value>(response, 16_384).await.ok();
				return Err(rejection(
					status,
					body.as_ref(),
					self.config.provider_credential.is_some(),
				));
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

/// The OpenRouter chat body. `transforms: []` disables OpenRouter's
/// middle-out compression so only the Agent's Context Policy reduces context.
/// The transmitted chat body. A salted request's Tenant Cache Salt line
/// starts `system`; it never enters request metadata or accounting.
fn request_body(
	config: &ModelConfig,
	request: &ModelRequest,
	media_routes: Vec<String>,
	salt: Option<&str>,
) -> Value {
	let mut body = request.input_body();
	if let Some(salt) = salt {
		let system = &mut body["messages"][0]["content"];
		let salted = format!("{salt}{}", system.as_str().unwrap_or_default());
		*system = Value::String(salted);
	}
	body["model"] = json!(config.model_id);
	body["max_tokens"] = json!(request.max_output_tokens);
	body["transforms"] = json!([]);
	// Enforce ZDR on every call, including existing registered models. Never
	// retry against non-ZDR endpoints if no eligible provider is available.
	body["provider"] = if media_routes.is_empty() {
		json!({"zdr": true, "require_parameters": true})
	} else {
		json!({"zdr": true, "require_parameters": true, "only": media_routes, "allow_fallbacks": true})
	};
	if let Some(effort) = config.reasoning_effort {
		body["reasoning"] = json!({"effort": effort});
	}
	body
}

/// Classifies a non-2xx chat reply. The upstream body is never exposed.
/// Only a body that reports a context-length limit proves an overflow; a bare
/// `413` can also be a media payload limit, which compaction cannot fix.
fn rejection(status: u16, body: Option<&Value>, byok: bool) -> Error {
	if matches!(status, 400 | 413) && body.is_some_and(context_overflow) {
		return Error::ContextOverflow;
	}
	crate::response::rejection(status, body, byok)
}

/// Provider-proven Context Overflow, as reported by OpenRouter ("This endpoint's
/// maximum context length is N tokens...") or relayed upstream wording.
fn context_overflow(body: &Value) -> bool {
	const PATTERNS: [&str; 5] = [
		"maximum context length",
		"context_length_exceeded",
		"context length exceeded",
		"prompt is too long",
		"exceeds the context window",
	];
	["/error/code", "/error/message", "/error/metadata/raw"]
		.iter()
		.filter_map(|pointer| body.pointer(pointer).and_then(Value::as_str))
		.any(|text| {
			let text = text.to_ascii_lowercase();
			PATTERNS.iter().any(|pattern| text.contains(pattern))
		})
}

pub(crate) fn safe_upstream_reason(detail: &str) -> String {
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
	let message = &choice["message"];
	// Truncated or refused output is never acted on, so no tool call escapes.
	if !message["refusal"].is_null() {
		return Err(Error::Context(Failure::Refused));
	}
	match choice["finish_reason"].as_str() {
		Some("stop" | "tool_calls") => {}
		Some("length") => return Err(Error::Context(Failure::OutputTruncated)),
		Some("content_filter") => return Err(Error::Context(Failure::Refused)),
		_ => {
			return Err(Error::External(
				"provider returned an unexpected finish reason".into(),
			));
		}
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
