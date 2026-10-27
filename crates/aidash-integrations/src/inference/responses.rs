//! Responses API transport for ChatGPT plans and OpenRouter Provider Credentials.
//!
//! Plan usage requires `stream: true` and `store: false` and keeps no
//! conversation state, so every request carries the whole projected context
//! in `input`. Legacy, Ordered and Native projections retain the observable
//! messages and tool turns without provider reasoning (ADR 0083). Output is
//! accepted only from `response.completed`; truncated terminal responses retain
//! text for bounded MCP sampling. Deltas feed sanitized Inference Progress.
use super::{CacheSaltKeys, MAX_RESPONSE_BYTES, safe_upstream_reason};
use crate::{Error, Result};
use aidash_application::ports::{InferenceProgressSink, ModelProvider};
use aidash_application::provider_access::{Context, Inference, Operation, ProviderAccess, Source};
use aidash_domain::provider::responses::{self as input, TOOL_NAMESPACE};
use aidash_domain::{
	context::recovery::Failure as ContextFailure,
	model::ModelConfig,
	plan_connections::{Disposition, Failure},
	provider::{
		ContentPart, ModelRequest, ModelResponse, ToolCall,
		progress::{InferenceProgress, MAX_PROGRESS_ITEM_BYTES},
		usage::{ProviderCost, ReportedUsage},
	},
};
use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The whole stream may carry this many times the assembled limit.
const STREAM_ENVELOPE_FACTOR: usize = 128;

pub struct ResponsesProvider {
	pub clients: crate::keyless::Clients,
	pub config: ModelConfig,
	pub access: Arc<dyn ProviderAccess>,
	pub context: Context,
	pub cache_salt: Option<CacheSaltKeys>,
}

impl ResponsesProvider {
	fn call_context(&self, max_output_tokens: u32) -> Context {
		let mut context = self.context.clone();
		context.inference = Some(Inference {
			model: self.config.model_id.clone(),
			operations: vec![Operation::Chat],
			max_output_tokens,
			cache_breakpoints: false,
			call: None,
			hosts: vec![],
		});
		context
	}
	fn is_plan(&self) -> bool {
		self.config.uses_plan_connection()
	}
	/// Plan usage is a Plan Connection; every other Responses route is an
	/// OpenRouter model served by the Tenant's Provider Credential.
	fn source(&self) -> Source {
		if let Some(provider) = &self.config.plan_connection {
			return Source::Plan {
				provider: provider.clone(),
			};
		}
		match &self.config.provider_credential {
			Some(provider) => Source::Tenant {
				provider: provider.clone(),
			},
			None => Source::Keyless,
		}
	}
}

/// A classified plan rejection and whether the access layer must act on it.
pub(crate) struct PlanRejection {
	pub error: Error,
	pub disposition: Option<Disposition>,
}

/// Classifies a non-2xx plan reply. Bodies may be `{"detail": "..."}` before
/// a stream opens, so the status decides when no code is present. Provider
/// text never leaves this function.
pub(crate) fn plan_rejection(status: u16, body: Option<&Value>) -> PlanRejection {
	let error = body
		.and_then(|body| body.get("error"))
		.filter(|e| e.is_object());
	let code = error
		.and_then(|error| error.get("code"))
		.and_then(Value::as_str);
	let param = error
		.and_then(|error| error.get("param"))
		.and_then(Value::as_str);
	if matches!(status, 400 | 413)
		&& let Some(body) = body
		&& (code == Some("context_length_exceeded") || super::context_overflow(body))
	{
		return PlanRejection {
			error: Error::ContextOverflow,
			disposition: None,
		};
	}
	let disposition = aidash_domain::plan_connections::classify(status, code, param);
	let error = match disposition {
		Disposition::Fail(failure) => Error::Plan(failure),
		Disposition::MarkIneligible => Error::Plan(Failure::Ineligible),
		Disposition::RefreshThenReconnect => Error::Plan(Failure::NeedsReconnection),
		// Retried as transport with the usual bounded backoff.
		Disposition::Transient => plan_unavailable(),
	};
	PlanRejection {
		error,
		disposition: Some(disposition),
	}
}

fn plan_unavailable() -> Error {
	Error::External("ChatGPT plan usage is temporarily unavailable".into())
}

/// The model-visible Responses body is shared with request accounting. The
/// adapter adds only route controls, model configuration and the Tenant salt.
/// This is the Plan Connection body and stays byte-identical (ADR 0007).
pub(crate) fn request_body(
	config: &ModelConfig,
	request: &ModelRequest,
	salt: Option<&str>,
) -> Result<Value> {
	if request
		.content_parts
		.iter()
		.any(|part| matches!(part, ContentPart::Audio { .. }))
	{
		return Err(Error::Plan(Failure::Unsupported(
			aidash_domain::plan_connections::Unsupported::Input,
		)));
	}
	let mut body = input::encoded_input_body(request)?;
	body["model"] = json!(config.model_id);
	body["instructions"] = json!(format!(
		"{}{}",
		salt.unwrap_or_default(),
		request.instructions
	));
	// Plan usage requires both; neither is configurable (ADR 0007).
	body["store"] = json!(false);
	body["stream"] = json!(true);
	if let Some(effort) = config.reasoning_effort {
		body["reasoning"] = json!({"effort": effort});
	}
	Ok(body)
}

/// OpenRouter's Responses body. It uses flat function tools and no namespace
/// on calls, keeps the same provider preferences as the chat body, and never
/// sends conversation state or a previous response ID. Streaming
/// is added by the caller only when the model streams.
fn openrouter_request_body(
	config: &ModelConfig,
	request: &ModelRequest,
	salt: Option<&str>,
	media_routes: Vec<String>,
) -> Result<Value> {
	let mut body = input::encoded_openrouter_input_body(request)?;
	body["model"] = json!(config.model_id);
	body["instructions"] = json!(format!(
		"{}{}",
		salt.unwrap_or_default(),
		request.instructions
	));
	body["max_output_tokens"] = json!(request.max_output_tokens);
	if request.disable_provider_transforms {
		body["truncation"] = json!("disabled");
	}
	body["provider"] = super::provider_preferences(config, media_routes, Default::default());
	if let Some(effort) = config.reasoning_effort {
		body["reasoning"] = json!({"effort": effort});
	}
	Ok(body)
}

#[async_trait]
impl ModelProvider for ResponsesProvider {
	async fn infer(
		&self,
		request: ModelRequest,
		progress: &dyn InferenceProgressSink,
	) -> Result<ModelResponse> {
		let deadline = self.config.request_timeout()?;
		let stall = self.config.stream_stall_timeout()?;
		tokio::time::timeout(deadline, async {
			request.validate()?;
			if request.cache_breakpoints {
				return Err(Error::Invalid(format!(
					"model {} does not accept cache breakpoints",
					self.config.model_id
				)));
			}
			if self.is_plan() {
				self.infer_plan(&request, progress, deadline, stall).await
			} else {
				self.infer_openrouter(&request, progress, deadline, stall)
					.await
			}
		})
		.await
		.map_err(|_| Error::External("model inference timed out".into()))?
	}
}

impl ResponsesProvider {
	/// OpenAI's plan Responses API, with the Plan Connection refresh rules.
	async fn infer_plan(
		&self,
		request: &ModelRequest,
		progress: &dyn InferenceProgressSink,
		deadline: std::time::Duration,
		stall: std::time::Duration,
	) -> Result<ModelResponse> {
		// The output limit is enforced locally: plan usage ignores
		// `max_output_tokens` (to verify, #159).
		if request.max_output_tokens > self.config.output_token_limit() {
			return Err(Error::Invalid(
				"plan inference exceeds the configured output limit".into(),
			));
		}
		for part in &request.content_parts {
			let modality = match part {
				ContentPart::Image { .. } => "image",
				ContentPart::Audio { .. } => {
					return Err(Error::Plan(Failure::Unsupported(
						aidash_domain::plan_connections::Unsupported::Input,
					)));
				}
				ContentPart::Text(_) => continue,
			};
			if !self.config.modalities.iter().any(|value| value == modality) {
				return Err(Error::Invalid(format!(
					"model {} does not support {modality} input",
					self.config.model_id
				)));
			}
		}
		let salt = salt_line(self, request)?;
		let body = request_body(&self.config, request, salt.as_deref())?;
		let context = self.call_context(request.max_output_tokens);
		let source = self.source();
		// OpenAI may reject an access token that has not expired; one
		// forced refresh may follow before the owner is asked to reconnect.
		for attempt in 0..2 {
			let access = self
				.access
				.resolve(&context, &self.config.endpoint, &source)
				.await?;
			let client = self.clients.select(&access)?;
			let started = std::time::Instant::now();
			let response = tokio::time::timeout(
				stall,
				client
					.post(format!(
						"{}/responses",
						access.endpoint.trim_end_matches('/')
					))
					.timeout(deadline)
					.bearer_auth(access.bearer.expose_secret())
					.json(&body)
					.send(),
			)
			.await
			.map_err(|_| Error::InferenceStalled)?
			.map_err(crate::keyless::send_error)?;
			metrics::histogram!("aidash_model_response_headers_seconds")
				.record(started.elapsed().as_secs_f64());
			if response.status().is_success() {
				let result = within_output_cap(
					read_stream(response, progress, stall, false).await,
					request.max_output_tokens,
				);
				// A rejected bearer reported after the stream opened still
				// needs its refresh or reconnection. Progress was shown, so
				// this call is not replayed; a refreshed token serves the
				// next attempt.
				let disposition = match result {
					Err(Error::Plan(Failure::Ineligible)) => Some(Disposition::MarkIneligible),
					Err(Error::Plan(Failure::NeedsReconnection)) => {
						Some(Disposition::RefreshThenReconnect)
					}
					_ => None,
				};
				if let Some(disposition) = disposition
					&& self.access.plan_rejected(&context, disposition).await?
				{
					return Err(plan_unavailable());
				}
				return result;
			}
			let status = response.status().as_u16();
			let body = crate::response::json::<Value>(response, 16_384).await.ok();
			let rejection = plan_rejection(status, body.as_ref());
			match rejection.disposition {
				Some(Disposition::RefreshThenReconnect) if attempt == 0 => {
					if self
						.access
						.plan_rejected(&context, Disposition::RefreshThenReconnect)
						.await?
					{
						continue;
					}
				}
				// OpenAI asks for a new sign-in only after a terminal refresh
				// error or a confirmed revocation; a freshly refreshed token's
				// rejection is neither, so it is retried as unavailability.
				Some(Disposition::RefreshThenReconnect) => return Err(plan_unavailable()),
				Some(Disposition::MarkIneligible) => {
					self.access
						.plan_rejected(&context, Disposition::MarkIneligible)
						.await?;
				}
				_ => {}
			}
			return Err(rejection.error);
		}
		Err(plan_unavailable())
	}

	/// OpenRouter's Responses API, authenticated by the Tenant's Provider
	/// Credential. It shares the chat transport's route checks, provider
	/// preferences, salt and rejection mapping, and reports usage and cost.
	async fn infer_openrouter(
		&self,
		request: &ModelRequest,
		progress: &dyn InferenceProgressSink,
		deadline: std::time::Duration,
		stall: std::time::Duration,
	) -> Result<ModelResponse> {
		let version = request.context.projection_version();
		if !version.is_legacy() && !self.config.supports_projection(version) {
			return Err(Error::UnsupportedProjection {
				model: self.config.model_id.clone(),
				version,
			});
		}
		if self.config.provider_credential.is_some()
			&& request.max_output_tokens > self.config.output_token_limit()
		{
			return Err(Error::Invalid(
				"BYOK inference exceeds the configured output limit".into(),
			));
		}
		for part in &request.content_parts {
			let modality = match part {
				ContentPart::Image { .. } => Some("image"),
				ContentPart::Audio { .. } => {
					return Err(Error::Invalid(
						"The OpenRouter Responses adapter does not accept audio input".into(),
					));
				}
				ContentPart::Text(_) => None,
			};
			if let Some(modality) = modality
				&& !self.config.modalities.iter().any(|value| value == modality)
			{
				return Err(Error::Invalid(format!(
					"model {} does not support {modality} input",
					self.config.model_id
				)));
			}
		}
		let salt = salt_line(self, request)?;
		let media_routes = super::verified_routes(
			&self.clients,
			&self.config,
			&*self.access,
			&self.context,
			request,
		)
		.await?;
		let mut body =
			openrouter_request_body(&self.config, request, salt.as_deref(), media_routes)?;
		let streaming = self.config.streaming();
		if streaming {
			body["stream"] = json!(true);
		}
		let access = self
			.access
			.resolve(
				&self.call_context(request.max_output_tokens),
				&self.config.endpoint,
				&self.source(),
			)
			.await?;
		let mut call = self
			.clients
			.select(&access)?
			.post(format!(
				"{}/responses",
				access.endpoint.trim_end_matches('/')
			))
			.timeout(deadline)
			.json(&body);
		if !access.bearer.expose_secret().is_empty() {
			call = call.bearer_auth(access.bearer.expose_secret());
		}
		let started = std::time::Instant::now();
		let response = if streaming {
			tokio::time::timeout(stall, call.send())
				.await
				.map_err(|_| Error::InferenceStalled)?
		} else {
			call.send().await
		}
		.map_err(crate::keyless::send_error)?;
		metrics::histogram!("aidash_model_response_headers_seconds")
			.record(started.elapsed().as_secs_f64());
		if !response.status().is_success() {
			let status = response.status().as_u16();
			let body = crate::response::json::<Value>(response, 16_384).await.ok();
			return Err(super::rejection(
				status,
				body.as_ref(),
				self.config.provider_credential.is_some(),
			));
		}
		let result = if streaming {
			read_stream(response, progress, stall, true).await
		} else {
			parse_response(&crate::response::bytes(response, MAX_RESPONSE_BYTES).await?)
		};
		record_token_metrics(&result);
		result
	}
}

/// Resolve the Tenant Cache Salt line before any provider traffic: a node
/// without the requested key version sends nothing rather than an unsalted
/// request.
fn salt_line(provider: &ResponsesProvider, request: &ModelRequest) -> Result<Option<String>> {
	request
		.cache_scope
		.as_ref()
		.map(|scope| {
			provider
				.cache_salt
				.as_ref()
				.ok_or_else(|| {
					Error::Invalid(format!(
						"Cache Salt Key v{} is not configured on this node",
						scope.key_version
					))
				})?
				.line(scope)
		})
		.transpose()
}

fn parse_response(body: &[u8]) -> Result<ModelResponse> {
	let value: Value = serde_json::from_slice(body)
		.map_err(|error| Error::External(format!("invalid response JSON: {error}")))?;
	let cost = super::provider_cost(body);
	match value["status"].as_str() {
		Some("incomplete") => incomplete(&value, cost, true),
		Some("failed") => Err(openrouter_stream_failure(value.get("error"))),
		_ if value.get("error").is_some_and(Value::is_object) => {
			Err(openrouter_stream_failure(value.get("error")))
		}
		_ => completed_with_cost(&value, cost, true),
	}
}

/// Token metrics match the Chat Completions path: a billed response counts,
/// a failure with no reported usage does not.
fn record_token_metrics(result: &Result<ModelResponse>) {
	if let Some(usage) = super::billed_usage(result) {
		for (direction, tokens) in [
			("input", usage.reported.input_tokens),
			("output", usage.reported.output_tokens),
			("input_cached_read", usage.reported.cache_read_tokens),
			("input_cached_write", usage.reported.cache_write_tokens),
		] {
			if let Some(tokens) = tokens {
				metrics::counter!("aidash_model_tokens_total", "direction" => direction)
					.increment(tokens);
			}
		}
	}
}

/// Plan usage ignores `max_output_tokens`, so the limit is enforced on the
/// reported output: a longer response is never acted on, and only its usage
/// settles the call.
pub(crate) fn within_output_cap(result: Result<ModelResponse>, cap: u32) -> Result<ModelResponse> {
	let (mut response, truncated) = match result {
		Ok(response) => (response, false),
		Err(Error::TerminalResponse(ContextFailure::OutputTruncated, response)) => {
			(*response, true)
		}
		Err(error) => return Err(error),
	};
	let exceeded = response
		.reported
		.output_tokens
		.is_some_and(|n| n > u64::from(cap));
	if exceeded {
		response.text.clear();
		response.tool_calls.clear();
	}
	if truncated || exceeded {
		Err(Error::TerminalResponse(
			ContextFailure::OutputTruncated,
			Box::new(response),
		))
	} else {
		Ok(response)
	}
}

async fn read_stream(
	mut response: reqwest::Response,
	progress: &dyn InferenceProgressSink,
	stall: std::time::Duration,
	openrouter: bool,
) -> Result<ModelResponse> {
	match super::stream::media_type(&response) {
		super::stream::MediaType::EventStream => {}
		// OpenRouter may answer a streamed request with one JSON response.
		super::stream::MediaType::Json if openrouter => {
			return parse_response(&crate::response::bytes(response, MAX_RESPONSE_BYTES).await?);
		}
		_ => {
			return Err(Error::External(
				if openrouter {
					"Responses API answered without an event stream"
				} else {
					"ChatGPT plan usage answered without an event stream"
				}
				.into(),
			));
		}
	}
	let mut assembler = if openrouter {
		Assembler::openrouter(progress, MAX_RESPONSE_BYTES)
	} else {
		Assembler::new(progress, MAX_RESPONSE_BYTES)
	};
	let mut last_data = tokio::time::Instant::now();
	while !assembler.done() {
		let chunk = tokio::time::timeout_at(last_data + stall, response.chunk())
			.await
			.map_err(|_| Error::InferenceStalled)?
			.map_err(crate::http_error)?;
		let Some(chunk) = chunk else {
			break;
		};
		if assembler.push(&chunk)? {
			last_data = tokio::time::Instant::now();
		}
	}
	assembler.finish()
}

/// Incrementally parses a Responses event stream. Text deltas and tool-call
/// identity become progress; the accepted output comes only from the final
/// `response.completed` event.
pub(crate) struct Assembler<'a> {
	progress: &'a dyn InferenceProgressSink,
	limit: usize,
	/// OpenRouter reports usage cost and uses its own failure table.
	openrouter: bool,
	line: Vec<u8>,
	pending_cr: bool,
	at_start: bool,
	data: Option<Vec<u8>>,
	received: usize,
	streamed: usize,
	calls: BTreeMap<u64, (Option<String>, Option<String>, u64)>,
	finished: Option<Result<ModelResponse>>,
}

impl<'a> Assembler<'a> {
	pub(crate) fn new(progress: &'a dyn InferenceProgressSink, limit: usize) -> Self {
		Self::with_mode(progress, limit, false)
	}

	pub(crate) fn openrouter(progress: &'a dyn InferenceProgressSink, limit: usize) -> Self {
		Self::with_mode(progress, limit, true)
	}

	fn with_mode(progress: &'a dyn InferenceProgressSink, limit: usize, openrouter: bool) -> Self {
		Self {
			progress,
			limit,
			openrouter,
			line: Vec::new(),
			pending_cr: false,
			at_start: true,
			data: None,
			received: 0,
			streamed: 0,
			calls: BTreeMap::new(),
			finished: None,
		}
	}

	/// Whether a terminal event (`completed`, `incomplete`, `failed` or
	/// `error`) arrived.
	pub(crate) fn done(&self) -> bool {
		self.finished.is_some()
	}

	/// Consumes one network chunk; returns whether it carried event data.
	/// Comments and empty `data:` fields never count as liveness.
	pub(crate) fn push(&mut self, mut chunk: &[u8]) -> Result<bool> {
		let stream_limit = self.limit.saturating_mul(STREAM_ENVELOPE_FACTOR);
		if chunk.len() > stream_limit.saturating_sub(self.received) {
			return Err(Error::External(format!(
				"response stream exceeds {stream_limit} bytes"
			)));
		}
		self.received += chunk.len();
		let mut live = false;
		while !chunk.is_empty() && !self.done() {
			if std::mem::take(&mut self.pending_cr) && chunk[0] == b'\n' {
				chunk = &chunk[1..];
				continue;
			}
			let end = chunk.iter().position(|byte| matches!(byte, b'\n' | b'\r'));
			let part = &chunk[..end.unwrap_or(chunk.len())];
			if part.len() > self.limit.saturating_sub(self.line.len()) {
				return Err(self.overflow());
			}
			self.line.extend_from_slice(part);
			let Some(end) = end else {
				// A data line still arriving is liveness: the stream ends with
				// one `response.completed` event carrying the whole response,
				// which can take longer than the stall timeout to receive.
				live |= !part.is_empty() && self.line.starts_with(b"data");
				break;
			};
			self.pending_cr = chunk[end] == b'\r';
			chunk = &chunk[end + 1..];
			let line = std::mem::take(&mut self.line);
			live |= self.field(&line)?;
		}
		Ok(live)
	}

	pub(crate) fn finish(self) -> Result<ModelResponse> {
		self.finished.unwrap_or_else(|| {
			Err(Error::External(
				"provider stream ended before completion".into(),
			))
		})
	}

	fn overflow(&self) -> Error {
		Error::External(format!("response exceeds {} bytes", self.limit))
	}

	fn field(&mut self, mut line: &[u8]) -> Result<bool> {
		if std::mem::take(&mut self.at_start) {
			line = line.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(line);
		}
		if line.is_empty() {
			if let Some(data) = self.data.take().filter(|data| !data.is_empty()) {
				self.event(&data)?;
				return Ok(true);
			}
			return Ok(false);
		}
		if line[0] == b':' {
			return Ok(false);
		}
		let (name, value) = match line.iter().position(|byte| *byte == b':') {
			Some(colon) => {
				let value = &line[colon + 1..];
				(&line[..colon], value.strip_prefix(b" ").unwrap_or(value))
			}
			None => (line, &[][..]),
		};
		// The `event:` field repeats the JSON `type`; only `data` is read.
		if name != b"data" {
			return Ok(false);
		}
		let data = self.data.get_or_insert_with(Vec::new);
		let separator = usize::from(!data.is_empty());
		if value.len() + separator > self.limit.saturating_sub(data.len()) {
			return Err(self.overflow());
		}
		if separator == 1 {
			data.push(b'\n');
		}
		data.extend_from_slice(value);
		Ok(!value.is_empty())
	}

	fn event(&mut self, data: &[u8]) -> Result<()> {
		if data == b"[DONE]" {
			return Ok(());
		}
		let event: Value = serde_json::from_slice(data)
			.map_err(|error| Error::External(format!("invalid response JSON: {error}")))?;
		match event["type"].as_str() {
			// OpenRouter also emits `response.content_part.delta` for text.
			Some("response.output_text.delta") => {
				if let Some(text) = event["delta"].as_str() {
					self.grow(text.len())?;
					offer_text(self.progress, text);
				}
			}
			Some("response.content_part.delta") if self.openrouter => {
				if let Some(text) = event["delta"].as_str() {
					self.grow(text.len())?;
					offer_text(self.progress, text);
				}
			}
			Some("response.output_item.added") => {
				let item = &event["item"];
				if item["type"] == "function_call" {
					let index = event["output_index"].as_u64().unwrap_or(0);
					let id = item["call_id"].as_str().map(str::to_owned);
					let name = item["name"].as_str().map(str::to_owned).map(tool_name);
					// A new call also costs its structure, so calls without an
					// identity under distinct indices still reach the cap.
					let structure = if self.calls.contains_key(&index) {
						0
					} else {
						super::stream::TOOL_CALL_STRUCTURE_BYTES
					};
					self.grow(
						id.as_ref().map_or(0, String::len)
							+ name.as_ref().map_or(0, String::len)
							+ structure,
					)?;
					self.calls.insert(index, (id, name, 0));
					self.offer_call(index);
				}
			}
			Some("response.function_call_arguments.delta") => {
				let index = event["output_index"].as_u64().unwrap_or(0);
				if let Some(delta) = event["delta"].as_str() {
					self.grow(delta.len())?;
					if let Some(call) = self.calls.get_mut(&index) {
						call.2 += delta.len() as u64;
					}
					self.offer_call(index);
				}
			}
			// OpenRouter names its terminal success event `response.done` in
			// parts of its documentation; accept both spellings.
			Some("response.completed") => {
				let cost = self
					.openrouter
					.then(|| super::response_provider_cost(data))
					.flatten();
				self.finished = Some(completed_with_cost(
					&event["response"],
					cost,
					self.openrouter,
				));
			}
			Some("response.done") if self.openrouter => {
				let cost = self
					.openrouter
					.then(|| super::response_provider_cost(data))
					.flatten();
				self.finished = Some(completed_with_cost(&event["response"], cost, true));
			}
			Some("response.incomplete") => {
				let response = &event["response"];
				let cost = self
					.openrouter
					.then(|| super::response_provider_cost(data))
					.flatten();
				self.finished = Some(incomplete(response, cost, self.openrouter));
			}
			Some("response.failed") => {
				let error = event.pointer("/response/error").cloned();
				self.finished = Some(Err(if self.openrouter {
					openrouter_stream_failure(error.as_ref())
				} else {
					stream_failure(error.as_ref())
				}));
			}
			Some("error") => {
				self.finished = Some(Err(if self.openrouter {
					openrouter_stream_failure(event.get("error").or(Some(&event)))
				} else {
					stream_failure(Some(&event))
				}));
			}
			// Lifecycle, reasoning and other events are liveness only.
			_ => {}
		}
		Ok(())
	}

	fn offer_call(&self, index: u64) {
		let Some((id, name, bytes)) = self.calls.get(&index) else {
			return;
		};
		let display =
			32 + id.as_ref().map_or(0, String::len) + name.as_ref().map_or(0, String::len)
				<= MAX_PROGRESS_ITEM_BYTES / 2;
		self.progress.offer(InferenceProgress::ToolCall {
			index: u32::try_from(index).unwrap_or(u32::MAX),
			id: id.clone().filter(|_| display),
			name: name.clone().filter(|_| display),
			argument_bytes: *bytes,
		});
	}

	fn grow(&mut self, bytes: usize) -> Result<()> {
		if bytes > self.limit.saturating_sub(self.streamed) {
			return Err(self.overflow());
		}
		self.streamed += bytes;
		Ok(())
	}
}

/// The Aidash tool name of a call into [`TOOL_NAMESPACE`]. Calls carry the
/// namespace in its own field; a qualified name is accepted as well.
fn tool_name(name: String) -> String {
	match name
		.strip_prefix(TOOL_NAMESPACE)
		.and_then(|rest| rest.strip_prefix('.'))
	{
		Some(unqualified) => unqualified.to_owned(),
		None => name,
	}
}

/// A failure that arrived after the stream opened. It is classified like an
/// HTTP rejection, but nothing is retried: progress was already shown.
fn stream_failure(error: Option<&Value>) -> Error {
	let body = error.map(|error| json!({"error": error}));
	let status = match error
		.and_then(|error| error.get("code"))
		.and_then(Value::as_str)
	{
		// Classified like a rejected request, so the harness compacts and
		// retries instead of retrying the same oversized request.
		_ if body.as_ref().is_some_and(super::context_overflow) => 400,
		Some("subscription_sharing_usage_limit_exceeded") => 429,
		Some(code) if code.ends_with("_unavailable") => 503,
		_ => 502,
	};
	let rejection = plan_rejection(status, body.as_ref());
	match rejection.error {
		Error::External(_) => Error::External(safe_upstream_reason(
			error
				.and_then(|error| error.get("message"))
				.and_then(Value::as_str)
				.unwrap_or_default(),
		)),
		error => error,
	}
}

/// An OpenRouter failure that arrived after the stream opened. It is mapped
/// like a rejected request, but nothing is replayed because progress was
/// already shown.
fn openrouter_stream_failure(error: Option<&Value>) -> Error {
	let body = error.map(|error| json!({"error": error}));
	let code = error.and_then(|error| error.get("code"));
	// An explicit HTTP-equivalent code takes precedence over message heuristics,
	// just as it does for Chat Completions and pre-stream HTTP rejections.
	let status = code
		.and_then(Value::as_u64)
		.and_then(|code| u16::try_from(code).ok())
		.filter(|code| (400..=599).contains(code))
		.unwrap_or_else(|| match code.and_then(Value::as_str) {
			Some("rate_limit_exceeded") => 429,
			Some(code) if code.ends_with("_unavailable") => 503,
			_ if body.as_ref().is_some_and(super::context_overflow) => 400,
			_ => 502,
		});
	super::rejection(status, body.as_ref(), true)
}

fn incomplete(
	response: &Value,
	cost: Option<ProviderCost>,
	openrouter: bool,
) -> Result<ModelResponse> {
	let failure = match response
		.pointer("/incomplete_details/reason")
		.and_then(Value::as_str)
	{
		Some("max_output_tokens" | "max_messages") => ContextFailure::OutputTruncated,
		Some("content_filter") => ContextFailure::Refused,
		_ => {
			return Err(Error::External(
				"provider returned an incomplete response".into(),
			));
		}
	};
	let mut result = usage_only_with_cost(response, cost, openrouter);
	if failure == ContextFailure::OutputTruncated {
		// Only terminal text is eligible for bounded sampling; partial calls never escape.
		for item in response["output"].as_array().into_iter().flatten() {
			if item["type"] == "message" {
				for part in item["content"].as_array().into_iter().flatten() {
					if part["type"] == "output_text" {
						result
							.text
							.push_str(part["text"].as_str().unwrap_or_default());
					}
				}
			}
		}
	}
	Err(Error::TerminalResponse(failure, Box::new(result)))
}

fn reported(response: &Value, openrouter: bool) -> ReportedUsage {
	let count = |pointer: &str| response.pointer(pointer).and_then(Value::as_u64);
	ReportedUsage {
		input_tokens: count("/usage/input_tokens"),
		output_tokens: count("/usage/output_tokens"),
		cache_read_tokens: count("/usage/input_tokens_details/cached_tokens"),
		cache_write_tokens: openrouter
			.then(|| count("/usage/input_tokens_details/cache_write_tokens"))
			.flatten(),
		reasoning_tokens: count("/usage/output_tokens_details/reasoning_tokens"),
		// Plan usage is billed to the owner's plan: Aidash learns no cost.
		// OpenRouter's reported cost is attached by its transport.
		cost: None,
		provider_identity: super::provider_identity(response),
	}
}

fn usage_only_with_cost(
	response: &Value,
	cost: Option<ProviderCost>,
	openrouter: bool,
) -> ModelResponse {
	let mut reported = reported(response, openrouter);
	reported.cost = cost;
	ModelResponse {
		usage_complete: reported.input_tokens.is_some() && reported.output_tokens.is_some(),
		input_tokens: reported.input_tokens.unwrap_or(0),
		output_tokens: reported.output_tokens.unwrap_or(0),
		reported,
		..Default::default()
	}
}

/// The accepted result of a `response.completed` event, validated like a
/// non-streamed Chat Completions response.
#[cfg(test)]
pub(crate) fn completed(response: &Value) -> Result<ModelResponse> {
	completed_with_cost(response, None, false)
}

fn completed_with_cost(
	response: &Value,
	cost: Option<ProviderCost>,
	openrouter: bool,
) -> Result<ModelResponse> {
	if response["status"]
		.as_str()
		.is_some_and(|status| status != "completed")
	{
		return Err(Error::External(
			"provider completed a response that is not complete".into(),
		));
	}
	let usage = usage_only_with_cost(response, cost, openrouter);
	let output = response["output"]
		.as_array()
		.ok_or_else(|| Error::External("provider returned no output".into()))?;
	let mut result = usage.clone();
	let mut refused = false;
	for item in output {
		match item["type"].as_str() {
			Some("message") => {
				for part in item["content"].as_array().into_iter().flatten() {
					match part["type"].as_str() {
						Some("output_text") => {
							result
								.text
								.push_str(part["text"].as_str().unwrap_or_default());
						}
						Some("refusal") => refused = true,
						_ => {}
					}
				}
			}
			Some("function_call") => {
				let field = |key: &str| {
					item[key]
						.as_str()
						.map(str::to_owned)
						.ok_or_else(|| Error::External(format!("provider omitted {key}")))
				};
				result.tool_calls.push(ToolCall {
					id: field("call_id")?,
					name: tool_name(field("name")?),
					arguments: serde_json::from_str(&field("arguments")?).map_err(|error| {
						Error::External(format!("invalid provider tool arguments: {error}"))
					})?,
				});
			}
			// Provider reasoning is deliberately discarded (ADR 0083).
			_ => {}
		}
	}
	// Refused output is never acted on; only its usage settles the call.
	if refused {
		return Err(Error::TerminalResponse(
			ContextFailure::Refused,
			Box::new(usage),
		));
	}
	if result.text.trim().is_empty() {
		result.text.clear();
	}
	let mut ids = std::collections::HashSet::new();
	if result.text.is_empty() && result.tool_calls.is_empty() {
		return Err(Error::External("empty model response".into()));
	}
	for call in &result.tool_calls {
		if call.id.is_empty() || !ids.insert(&call.id) || !call.arguments.is_object() {
			return Err(Error::External(
				"invalid or duplicate model tool call".into(),
			));
		}
	}
	Ok(result)
}

/// Offer text in pieces that each fit a stored progress item.
fn offer_text(progress: &dyn InferenceProgressSink, mut text: &str) {
	const PIECE: usize = MAX_PROGRESS_ITEM_BYTES / 2;
	while !text.is_empty() {
		let mut end = text.len().min(PIECE);
		while !text.is_char_boundary(end) {
			end -= 1;
		}
		let (piece, rest) = text.split_at(end);
		progress.offer(InferenceProgress::Text {
			text: piece.to_owned(),
		});
		text = rest;
	}
}

#[cfg(test)]
mod tests;
