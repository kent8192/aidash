//! OpenRouter inference transport implementing the application port.
use crate::{Error, Result};
use aidash_application::ports::ModelProvider;
use aidash_application::provider_access::{Context, Inference, Operation, ProviderAccess, Source};
use aidash_domain::{
	model::ModelConfig,
	provider::{
		ContentPart, ModelRequest, ModelResponse, ToolCall,
		usage::{ProviderCost, ReportedUsage},
	},
};
use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde::Deserialize;
use serde_json::{Value, json, value::RawValue};
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
			// Unsupported routes never receive `cache_control` (ADR 0019).
			if request.cache_breakpoints && !self.config.accepts_cache_breakpoints() {
				return Err(Error::Invalid(format!(
					"model {} does not accept cache breakpoints",
					self.config.model_id
				)));
			}
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
			let mut body = request.input_body();
			if let Some(salt) = salt {
				// With a cache breakpoint, `system` is one marked text block; the
				// salt starts its text, so cached prefixes stay per Tenant.
				let system = match &mut body["messages"][0]["content"] {
					Value::Array(blocks) => &mut blocks[0]["text"],
					text => text,
				};
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
				return Err(crate::response::provider_rejection(
					response,
					self.config.provider_credential.is_some(),
				)
				.await);
			}
			let result = parse_openai(&crate::response::bytes(response, 1_048_576).await?)?;
			// Unknown usage is absent, never a zero sample.
			for (direction, tokens) in [
				("input", result.reported.input_tokens),
				("output", result.reported.output_tokens),
				("input_cached_read", result.reported.cache_read_tokens),
				("input_cached_write", result.reported.cache_write_tokens),
			] {
				if let Some(tokens) = tokens {
					metrics::counter!("aidash_model_tokens_total", "direction" => direction)
						.increment(tokens);
				}
			}
			Ok(result)
		})
		.await
		.map_err(|_| Error::External("model inference timed out".into()))?
	}
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

/// Only the cost fields OpenRouter reports in credits, kept as raw JSON text.
/// Without `serde_json`'s `arbitrary_precision`, a parsed `Value` would already
/// have rounded each decimal to the nearest `f64`.
#[derive(Deserialize)]
struct RawCosts<'a> {
	#[serde(borrow)]
	usage: Option<RawUsageCosts<'a>>,
}
#[derive(Deserialize)]
struct RawUsageCosts<'a> {
	#[serde(borrow)]
	cost: Option<&'a RawValue>,
	#[serde(borrow)]
	cost_details: Option<RawCostDetails<'a>>,
}
#[derive(Deserialize)]
struct RawCostDetails<'a> {
	#[serde(borrow)]
	upstream_inference_cost: Option<&'a RawValue>,
}

fn provider_cost(body: &[u8]) -> Option<ProviderCost> {
	// A cost field with an unexpected shape is unknown, not a parse failure.
	let usage = serde_json::from_slice::<RawCosts>(body).ok()?.usage?;
	ProviderCost::from_report(
		usage.cost.map(RawValue::get),
		usage
			.cost_details
			.and_then(|details| details.upstream_inference_cost)
			.map(RawValue::get),
	)
}

pub fn parse_openai(body: &[u8]) -> Result<ModelResponse> {
	let value: Value = serde_json::from_slice(body)
		.map_err(|error| Error::External(format!("invalid response JSON: {error}")))?;
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
	let count = |pointer: &str| value.pointer(pointer).and_then(Value::as_u64);
	let reported = ReportedUsage {
		input_tokens: count("/usage/prompt_tokens"),
		output_tokens: count("/usage/completion_tokens"),
		cache_read_tokens: count("/usage/prompt_tokens_details/cached_tokens"),
		cache_write_tokens: count("/usage/prompt_tokens_details/cache_write_tokens"),
		reasoning_tokens: count("/usage/completion_tokens_details/reasoning_tokens"),
		cost: provider_cost(body),
	};
	let mut result = ModelResponse {
		text: if content.trim().is_empty() {
			String::new()
		} else {
			content.to_owned()
		},
		usage_complete: reported.input_tokens.is_some() && reported.output_tokens.is_some(),
		input_tokens: reported.input_tokens.unwrap_or(0),
		output_tokens: reported.output_tokens.unwrap_or(0),
		reported,
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
