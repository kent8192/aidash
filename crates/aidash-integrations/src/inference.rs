//! OpenRouter inference transport implementing the application port.
use crate::{Error, Result};
use aidash_application::ports::{Credentials, ModelProvider};
use aidash_domain::{
	model::ModelConfig,
	provider::{
		ContentPart, ModelRequest, ModelResponse, ToolCall,
		usage::{ProviderCost, ReportedUsage},
	},
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json, value::RawValue};
use std::sync::Arc;

pub struct OpenRouterProvider {
	pub client: reqwest::Client,
	pub config: ModelConfig,
	pub credentials: Arc<dyn Credentials>,
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
) -> Result<Arc<dyn ModelProvider>> {
	match config.provider.as_str() {
		"openrouter" => {
			config.request_timeout()?;
			Ok(Arc::new(OpenRouterProvider {
				client,
				config,
				credentials,
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
