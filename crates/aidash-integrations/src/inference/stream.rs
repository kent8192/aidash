//! Assembly of an OpenAI-compatible server-sent completion stream.
//!
//! Deltas are accumulated into the same JSON shape as a non-streamed completion
//! and validated by `parse_openai`, so a streamed and a non-streamed response
//! with the same content are accepted or rejected identically. Only sanitized
//! progress (text, and tool-call identity plus argument size) is offered while
//! the stream is open; argument text and reasoning never leave the assembler.
use super::{parse_openai, safe_upstream_reason};
use crate::{Error, Result};
use aidash_application::ports::InferenceProgressSink;
use aidash_domain::provider::{
	ModelResponse,
	progress::{InferenceProgress, MAX_PROGRESS_ITEM_BYTES},
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// The whole stream may carry this many times the assembled limit. A response
/// that fits the limit can arrive as one-token deltas, each wrapped in a few
/// hundred bytes of chunk envelope; reasoning and unknown fields count too.
const STREAM_ENVELOPE_FACTOR: usize = 128;

#[derive(Default)]
struct PartialCall {
	id: Option<String>,
	name: Option<String>,
	arguments: Option<String>,
}

/// Incrementally parses SSE bytes and assembles one completion choice.
pub(crate) struct StreamAssembler<'a> {
	progress: &'a dyn InferenceProgressSink,
	limit: usize,
	line: Vec<u8>,
	data: Option<Vec<u8>>,
	received: usize,
	assembled: usize,
	content: String,
	refusal: Option<String>,
	calls: BTreeMap<u64, PartialCall>,
	finish_reason: Option<String>,
	usage: Option<Value>,
	done: bool,
}

impl<'a> StreamAssembler<'a> {
	/// `limit` bounds the assembled response and any single SSE line or event;
	/// every received byte counts toward `STREAM_ENVELOPE_FACTOR` times it.
	pub(crate) fn new(progress: &'a dyn InferenceProgressSink, limit: usize) -> Self {
		Self {
			progress,
			limit,
			line: Vec::new(),
			data: None,
			received: 0,
			assembled: 0,
			content: String::new(),
			refusal: None,
			calls: BTreeMap::new(),
			finish_reason: None,
			usage: None,
			done: false,
		}
	}

	/// Whether the terminal `[DONE]` event was received.
	pub(crate) fn done(&self) -> bool {
		self.done
	}

	/// Consume one network chunk. Returns whether it carried any `data:` field,
	/// which is the only liveness signal; comment lines never count.
	pub(crate) fn push(&mut self, mut chunk: &[u8]) -> Result<bool> {
		let stream_limit = self.limit.saturating_mul(STREAM_ENVELOPE_FACTOR);
		if chunk.len() > stream_limit.saturating_sub(self.received) {
			return Err(Error::External(format!(
				"response stream exceeds {stream_limit} bytes"
			)));
		}
		self.received += chunk.len();
		let mut live = false;
		while !chunk.is_empty() && !self.done {
			let (part, end) = match chunk.iter().position(|byte| *byte == b'\n') {
				Some(end) => (&chunk[..end], true),
				None => (chunk, false),
			};
			if part.len() > self.limit.saturating_sub(self.line.len()) {
				return Err(self.overflow());
			}
			self.line.extend_from_slice(part);
			chunk = &chunk[(part.len() + usize::from(end)).min(chunk.len())..];
			if end {
				let mut line = std::mem::take(&mut self.line);
				if line.last() == Some(&b'\r') {
					line.pop();
				}
				live |= self.field(&line)?;
				line.clear();
				self.line = line;
			}
		}
		Ok(live)
	}

	/// Validate the complete stream exactly like a non-streamed completion.
	pub(crate) fn finish(self) -> Result<ModelResponse> {
		if !self.done {
			return Err(Error::External(
				"provider stream ended before completion".into(),
			));
		}
		let Some(finish_reason) = self.finish_reason else {
			return Err(Error::External(
				"provider stream ended without a finish reason".into(),
			));
		};
		let mut message = Map::new();
		message.insert("content".into(), Value::String(self.content));
		message.insert(
			"refusal".into(),
			self.refusal.map_or(Value::Null, Value::String),
		);
		if !self.calls.is_empty() {
			let calls = self
				.calls
				.into_values()
				.map(|call| {
					let mut function = Map::new();
					if let Some(name) = call.name {
						function.insert("name".into(), Value::String(name));
					}
					if let Some(arguments) = call.arguments {
						function.insert("arguments".into(), Value::String(arguments));
					}
					let mut value = Map::new();
					if let Some(id) = call.id {
						value.insert("id".into(), Value::String(id));
					}
					value.insert("type".into(), json!("function"));
					value.insert("function".into(), Value::Object(function));
					Value::Object(value)
				})
				.collect();
			message.insert("tool_calls".into(), Value::Array(calls));
		}
		let mut completion = json!({
			"choices": [{"finish_reason": finish_reason, "message": message}],
		});
		if let Some(usage) = self.usage {
			completion["usage"] = usage;
		}
		parse_openai(completion)
	}

	fn overflow(&self) -> Error {
		Error::External(format!("response exceeds {} bytes", self.limit))
	}

	fn field(&mut self, line: &[u8]) -> Result<bool> {
		if line.is_empty() {
			// An empty data buffer dispatches nothing, as in the SSE specification.
			if let Some(data) = self.data.take().filter(|data| !data.is_empty()) {
				self.event(&data)?;
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
		Ok(true)
	}

	fn event(&mut self, data: &[u8]) -> Result<()> {
		if data == b"[DONE]" {
			self.done = true;
			return Ok(());
		}
		let chunk: Value = serde_json::from_slice(data)
			.map_err(|error| Error::External(format!("invalid response JSON: {error}")))?;
		if let Some(error) = chunk.get("error").filter(|error| !error.is_null()) {
			return Err(rejected(error));
		}
		if let Some(choices) = chunk.get("choices").and_then(Value::as_array) {
			// A non-streamed body is read at `/choices/0`. An omitted index can
			// only identify the sole choice of a chunk; with several, it is ambiguous.
			let unindexed = |choice: &Value| choice.get("index").is_none_or(Value::is_null);
			if choices.len() > 1 && choices.iter().any(unindexed) {
				return Err(Error::External(
					"provider stream returned ambiguous unindexed choices".into(),
				));
			}
			for choice in choices
				.iter()
				.filter(|choice| unindexed(choice) || choice["index"] == 0)
			{
				self.choice(choice)?;
			}
		}
		if let Some(usage) = chunk.get("usage").filter(|usage| usage.is_object()) {
			self.usage = Some(usage.clone());
		}
		Ok(())
	}

	fn choice(&mut self, choice: &Value) -> Result<()> {
		if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
			if reason == "error" {
				return Err(rejected(choice.get("error").unwrap_or(&Value::Null)));
			}
			self.finish_reason = Some(reason.to_owned());
		}
		let Some(delta) = choice.get("delta") else {
			return Ok(());
		};
		// Reasoning fields are dropped; their arrival already counted as liveness.
		if let Some(text) = delta.get("content").and_then(Value::as_str) {
			self.grow(text.len())?;
			self.content.push_str(text);
			offer_text(self.progress, text);
		}
		if let Some(text) = delta.get("refusal").and_then(Value::as_str) {
			self.grow(text.len())?;
			self.refusal.get_or_insert_with(String::new).push_str(text);
		}
		if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
			for (position, call) in calls.iter().enumerate() {
				self.tool_call(position, call)?;
			}
		}
		Ok(())
	}

	fn tool_call(&mut self, position: usize, fragment: &Value) -> Result<()> {
		let index = match fragment.get("index") {
			Some(index) => index
				.as_u64()
				.filter(|index| u32::try_from(*index).is_ok())
				.ok_or_else(|| Error::External("invalid streamed tool call index".into()))?,
			None => position as u64,
		};
		let id = fragment.get("id").and_then(Value::as_str);
		let function = fragment.get("function");
		let name = function
			.and_then(|function| function.get("name"))
			.and_then(Value::as_str);
		let arguments = function
			.and_then(|function| function.get("arguments"))
			.and_then(Value::as_str);
		let growth = [id, name, arguments]
			.into_iter()
			.flatten()
			.map(str::len)
			.sum();
		self.grow(growth)?;
		let call = self.calls.entry(index).or_default();
		set_identity(&mut call.id, id)?;
		set_identity(&mut call.name, name)?;
		if let Some(arguments) = arguments {
			call.arguments
				.get_or_insert_with(String::new)
				.push_str(arguments);
		}
		let argument_bytes = call.arguments.as_ref().map_or(0, String::len) as u64;
		// Identity is display data only; omit it rather than exceed an item bound.
		let display = 32
			+ call.id.as_ref().map_or(0, String::len)
			+ call.name.as_ref().map_or(0, String::len)
			<= MAX_PROGRESS_ITEM_BYTES / 2;
		self.progress.offer(InferenceProgress::ToolCall {
			index: index as u32,
			id: call.id.clone().filter(|_| display),
			name: call.name.clone().filter(|_| display),
			argument_bytes,
		});
		Ok(())
	}

	fn grow(&mut self, bytes: usize) -> Result<()> {
		if bytes > self.limit.saturating_sub(self.assembled) {
			return Err(self.overflow());
		}
		self.assembled += bytes;
		Ok(())
	}
}

/// A tool call's ID and name arrive once and may be repeated, never changed.
fn set_identity(slot: &mut Option<String>, value: Option<&str>) -> Result<()> {
	match (slot.as_deref(), value) {
		(_, None | Some("")) => Ok(()),
		(None | Some(""), Some(value)) => {
			*slot = Some(value.to_owned());
			Ok(())
		}
		(Some(current), Some(value)) if current == value => Ok(()),
		_ => Err(Error::External(
			"invalid or duplicate model tool call".into(),
		)),
	}
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

/// Normalize a mid-stream provider error like a non-streamed HTTP rejection.
fn rejected(error: &Value) -> Error {
	let status = error
		.get("code")
		.and_then(Value::as_u64)
		.and_then(|code| u16::try_from(code).ok())
		.filter(|code| (400..=599).contains(code))
		.unwrap_or(502);
	let detail = error
		.get("message")
		.and_then(Value::as_str)
		.unwrap_or("upstream rejected the request without a readable reason");
	Error::ProviderRejected {
		status,
		reason: safe_upstream_reason(detail),
	}
}

/// How a 2xx answer to a streamed request is encoded.
pub(crate) enum MediaType {
	EventStream,
	/// A whole completion from an endpoint that ignores `stream`.
	Json,
	Other,
}

pub(crate) fn media_type(response: &reqwest::Response) -> MediaType {
	let essence = response
		.headers()
		.get(reqwest::header::CONTENT_TYPE)
		.and_then(|value| value.to_str().ok())
		.and_then(|value| value.split(';').next())
		.map(str::trim);
	match essence {
		Some(essence) if essence.eq_ignore_ascii_case("text/event-stream") => {
			MediaType::EventStream
		}
		Some(essence) if essence.eq_ignore_ascii_case("application/json") => MediaType::Json,
		_ => MediaType::Other,
	}
}

#[cfg(test)]
mod tests;
