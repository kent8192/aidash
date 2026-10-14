//! OpenAI Responses API transport for ChatGPT plan usage (#159, ADR 0007).
//!
//! Plan usage requires `stream: true` and `store: false` and keeps no
//! conversation state, so every request carries the whole projected context
//! in `input`. Only the Legacy and Ordered Projection Versions are supported;
//! the Native version and reasoning items are #249. Output is accepted only
//! from `response.completed`; deltas feed sanitized Inference Progress.
use super::{CacheSaltKeys, MAX_RESPONSE_BYTES, safe_upstream_reason};
use crate::{Error, Result};
use aidash_application::ports::{InferenceProgressSink, ModelProvider};
use aidash_application::provider_access::{Context, Inference, Operation, ProviderAccess, Source};
use aidash_domain::{
	context::recovery::Failure as ContextFailure,
	model::ModelConfig,
	plan_connections::{Disposition, Failure},
	provider::{
		ContentPart, ModelRequest, ModelResponse, ToolCall,
		progress::{InferenceProgress, MAX_PROGRESS_ITEM_BYTES},
		usage::ReportedUsage,
	},
};
use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde_json::{Map, Value, json};
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
		});
		context
	}
	fn source(&self) -> Source {
		Source::configured(
			&self.config.provider_credential,
			&self.config.plan_connection,
		)
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
		Disposition::Transient => {
			Error::External("ChatGPT plan usage is temporarily unavailable".into())
		}
	};
	PlanRejection {
		error,
		disposition: Some(disposition),
	}
}

/// The namespace every Aidash tool is declared in. Plan usage accepts function
/// tools only inside a namespace (or as `additional_tools` input items), never
/// as flat top-level tools.
const TOOL_NAMESPACE: &str = "aidash";

/// The Responses request equivalent to the Chat Completions body every other
/// route receives, so context accounting stays the same: `system` becomes
/// `instructions`, the user message becomes one `input` message, and tools
/// become function tools in one [`TOOL_NAMESPACE`] namespace.
pub(crate) fn request_body(
	config: &ModelConfig,
	request: &ModelRequest,
	salt: Option<&str>,
) -> Result<Value> {
	let chat = request.input_body();
	let text = |value: &Value| -> String {
		match value {
			Value::String(text) => text.clone(),
			Value::Array(blocks) => blocks
				.iter()
				.filter_map(|block| block.get("text").and_then(Value::as_str))
				.collect(),
			_ => String::new(),
		}
	};
	let instructions = format!(
		"{}{}",
		salt.unwrap_or_default(),
		text(&chat["messages"][0]["content"])
	);
	let content = match &chat["messages"][1]["content"] {
		Value::String(text) => vec![json!({"type":"input_text","text":text})],
		Value::Array(parts) => parts
			.iter()
			.map(|part| match part["type"].as_str() {
				Some("text") => Ok(json!({"type":"input_text","text":part["text"]})),
				Some("image_url") => {
					Ok(json!({"type":"input_image","image_url":part["image_url"]["url"]}))
				}
				_ => Err(Error::Plan(Failure::Unsupported(
					aidash_domain::plan_connections::Unsupported::Input,
				))),
			})
			.collect::<Result<Vec<_>>>()?,
		_ => return Err(Error::Invalid("model request has no user message".into())),
	};
	let mut body = json!({
		"model": config.model_id,
		"instructions": instructions,
		"input": [{"role":"user","content":content}],
		// Plan usage requires both; neither is configurable (ADR 0007).
		"store": false,
		"stream": true,
	});
	if !request.tools.is_empty() {
		let tools = request
			.tools
			.iter()
			.map(|tool| {
				json!({"type":"function","name":tool.name,"description":tool.description,"parameters":tool.parameters})
			})
			.collect::<Vec<_>>();
		body["tools"] = json!([{
			"type": "namespace",
			"name": TOOL_NAMESPACE,
			"description": "Tools of this Aidash agent.",
			"tools": tools,
		}]);
	}
	if let Some(format) = &request.response_format {
		body["text"] = json!({"format": text_format(format)?});
	}
	if let Some(effort) = config.reasoning_effort {
		body["reasoning"] = json!({"effort": effort});
	}
	Ok(body)
}

/// Chat Completions' `response_format` in the Responses `text.format` shape.
fn text_format(format: &Value) -> Result<Value> {
	match format["type"].as_str() {
		Some("json_schema") => {
			let schema = &format["json_schema"];
			let mut value = Map::new();
			value.insert("type".into(), json!("json_schema"));
			for key in ["name", "schema", "strict", "description"] {
				if let Some(field) = schema.get(key) {
					value.insert(key.into(), field.clone());
				}
			}
			Ok(Value::Object(value))
		}
		Some("json_object") => Ok(json!({"type":"json_object"})),
		Some("text") => Ok(json!({"type":"text"})),
		_ => Err(Error::Invalid("unsupported response format".into())),
	}
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
			let body = request_body(&self.config, &request, salt.as_deref())?;
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
						read_stream(response, progress, stall).await,
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
						return Err(Error::External(
							"ChatGPT plan usage is temporarily unavailable".into(),
						));
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
					Some(Disposition::MarkIneligible) => {
						self.access
							.plan_rejected(&context, Disposition::MarkIneligible)
							.await?;
					}
					_ => {}
				}
				return Err(rejection.error);
			}
			Err(Error::Plan(Failure::NeedsReconnection))
		})
		.await
		.map_err(|_| Error::External("model inference timed out".into()))?
	}
}

/// Plan usage ignores `max_output_tokens`, so the limit is enforced on the
/// reported output: a longer response is never acted on, and only its usage
/// settles the call.
pub(crate) fn within_output_cap(result: Result<ModelResponse>, cap: u32) -> Result<ModelResponse> {
	match result {
		Ok(response)
			if response
				.reported
				.output_tokens
				.is_some_and(|n| n > u64::from(cap)) =>
		{
			Err(Error::TerminalResponse(
				ContextFailure::OutputTruncated,
				Box::new(ModelResponse {
					text: String::new(),
					tool_calls: Vec::new(),
					..response
				}),
			))
		}
		result => result,
	}
}

async fn read_stream(
	mut response: reqwest::Response,
	progress: &dyn InferenceProgressSink,
	stall: std::time::Duration,
) -> Result<ModelResponse> {
	if !matches!(
		super::stream::media_type(&response),
		super::stream::MediaType::EventStream
	) {
		return Err(Error::External(
			"ChatGPT plan usage answered without an event stream".into(),
		));
	}
	let mut assembler = Assembler::new(progress, MAX_RESPONSE_BYTES);
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
		Self {
			progress,
			limit,
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

	/// Consumes one network chunk; returns whether it dispatched a data event.
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
		Ok(false)
	}

	fn event(&mut self, data: &[u8]) -> Result<()> {
		if data == b"[DONE]" {
			return Ok(());
		}
		let event: Value = serde_json::from_slice(data)
			.map_err(|error| Error::External(format!("invalid response JSON: {error}")))?;
		match event["type"].as_str() {
			Some("response.output_text.delta") => {
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
					self.grow(
						id.as_ref().map_or(0, String::len) + name.as_ref().map_or(0, String::len),
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
			Some("response.completed") => {
				self.finished = Some(completed(&event["response"]));
			}
			Some("response.incomplete") => {
				let response = &event["response"];
				let failure = match response
					.pointer("/incomplete_details/reason")
					.and_then(Value::as_str)
				{
					Some("max_output_tokens") => ContextFailure::OutputTruncated,
					Some("content_filter") => ContextFailure::Refused,
					_ => {
						self.finished = Some(Err(Error::External(
							"provider returned an incomplete response".into(),
						)));
						return Ok(());
					}
				};
				let usage = usage_only(response);
				self.finished = Some(Err(Error::TerminalResponse(failure, Box::new(usage))));
			}
			Some("response.failed") => {
				let error = event.pointer("/response/error").cloned();
				self.finished = Some(Err(stream_failure(error.as_ref())));
			}
			Some("error") => {
				self.finished = Some(Err(stream_failure(Some(&event))));
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

fn reported(response: &Value) -> ReportedUsage {
	let count = |pointer: &str| response.pointer(pointer).and_then(Value::as_u64);
	ReportedUsage {
		input_tokens: count("/usage/input_tokens"),
		output_tokens: count("/usage/output_tokens"),
		cache_read_tokens: count("/usage/input_tokens_details/cached_tokens"),
		cache_write_tokens: None,
		reasoning_tokens: count("/usage/output_tokens_details/reasoning_tokens"),
		// Plan usage is billed to the owner's plan: Aidash learns no cost.
		cost: None,
	}
}

fn usage_only(response: &Value) -> ModelResponse {
	let reported = reported(response);
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
pub(crate) fn completed(response: &Value) -> Result<ModelResponse> {
	if response["status"]
		.as_str()
		.is_some_and(|status| status != "completed")
	{
		return Err(Error::External(
			"provider completed a response that is not complete".into(),
		));
	}
	let usage = usage_only(response);
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
			// Reasoning items are not round-tripped yet (#249).
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
