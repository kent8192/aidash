//! The Native Projection Version (ADR 0015, #178): a typed, provider-neutral
//! intermediate representation of one request. Recorded Model Turns become
//! assistant and tool messages instead of events inside one JSON user message.
//!
//! The harness builds this IR; `ModelRequest::input_body` gives it its
//! canonical chat serialization, which transport and request budgeting share.
use crate::context::{Context, ContextEvent, HistoryEntry, ORDERED_STABLE_KEYS};
use crate::provider::ToolCall;
use crate::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The only trust level of enveloped content: data, never instructions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Trust {
	Data,
}

/// Where enveloped content came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Source {
	Identity,
	Task,
	ReferenceDocuments,
	RunMessageSummary,
	ExecutionSummary,
	Tool,
	ModelMediaObservation,
	/// The question a Human answer replies to. It may quote model or tool
	/// output, so only the answer itself is unwrapped.
	HumanRequest,
	/// This step's volatile content.
	Step,
}

/// The provenance envelope around every piece of data-derived content in a
/// Native request. Human input is the only non-system instruction channel and
/// is never wrapped; everything else says where it came from and that it is
/// data. Field order is the serialized order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
	pub source: Source,
	pub provenance: Value,
	/// Revision of the source, or its Context Journal sequence; `null` when
	/// the source has none.
	pub revision: Option<u64>,
	pub trust: Trust,
	pub content: Value,
}

impl Envelope {
	fn data(source: Source, provenance: Value, revision: Option<u64>, content: Value) -> Self {
		Self {
			source,
			provenance,
			revision,
			trust: Trust::Data,
			content,
		}
	}

	/// Canonical text of the envelope: typed field order, sorted JSON maps.
	pub fn text(&self) -> String {
		serde_json::to_string(self).expect("serializable provenance envelope")
	}

	/// Envelope text around content that is already canonical JSON text, kept
	/// byte for byte so its key order survives.
	fn raw_text(source: Source, provenance: Value, content: &str) -> String {
		let head = Self::data(source, provenance, None, Value::Null).text();
		let head = head
			.strip_suffix("null}")
			.expect("content is the last envelope field");
		format!("{head}{content}}}")
	}
}

/// One history message of a Native request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeMessage {
	/// A Human answer, sent as a user message. The answer is the only
	/// non-system instruction channel and is not wrapped; the request it
	/// replies to is.
	Human { request: Envelope, response: Value },
	/// Data-derived history that is not a tool result, sent as an enveloped
	/// user message.
	Data { envelope: Envelope },
	/// The response that opened a Model Turn, with the calls it made.
	Assistant {
		#[serde(default, skip_serializing_if = "Option::is_none")]
		text: Option<String>,
		tool_calls: Vec<ToolCall>,
	},
	/// One call's result, paired with its call by `tool_call_id`.
	Tool {
		tool_call_id: String,
		envelope: Envelope,
	},
}

/// A complete Native request context: Run-stable attachments, then history
/// rendered as whole Model Turns, then this step's volatile content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeContext {
	/// Run-stable sources, one envelope each, sent in one user message right
	/// after `system`. They are part of the Stable Prefix.
	pub attachments: Vec<Envelope>,
	pub history: Vec<NativeMessage>,
	/// Canonical JSON of this step's volatile content, sent enveloped as the
	/// trailing user message. It is never stored in history.
	pub volatile: String,
}

/// Project a Run's context and this step's pinned snapshot into the Native IR.
///
/// Each `Assistant` event opens a Model Turn that lasts until the next one.
/// The turn's `Tool` events, whatever else the journal recorded between them,
/// are emitted right after its assistant message in call order; a Human
/// answer given during the turn (an approval) follows them. Calls that never
/// ran have no `Tool` event and are left out of the assistant message, as
/// Legacy leaves them out of history, so no request carries a partial Model
/// Turn. Continuation markers recorded since the last Model Turn move to the
/// volatile part; older ones are no longer current and are not rendered.
pub fn native_context(context: &Context, pinned: &Value) -> NativeContext {
	let mut history = Vec::new();
	let mut markers = Vec::new();
	let entries = &context.history;
	let mut index = 0;
	while index < entries.len() {
		let ContextEvent::Assistant { text, calls, .. } = &entries[index].event else {
			single(&entries[index], &mut history, &mut markers);
			index += 1;
			continue;
		};
		let end = entries[index + 1..]
			.iter()
			.position(|entry| matches!(entry.event, ContextEvent::Assistant { .. }))
			.map_or(entries.len(), |offset| index + 1 + offset);
		let mut results = BTreeMap::new();
		let mut after = Vec::new();
		for entry in &entries[index + 1..end] {
			match &entry.event {
				ContextEvent::Tool { call, .. }
					if calls.contains(&call.id) && !results.contains_key(call.id.as_str()) =>
				{
					results.insert(call.id.as_str(), entry);
				}
				_ => after.push(entry),
			}
		}
		let executed = calls
			.iter()
			.filter_map(|id| results.get(id.as_str()).copied())
			.collect::<Vec<_>>();
		let text = (!text.trim().is_empty()).then(|| text.clone());
		if text.is_some() || !executed.is_empty() {
			history.push(NativeMessage::Assistant {
				text,
				tool_calls: executed
					.iter()
					.filter_map(|entry| match &entry.event {
						ContextEvent::Tool { call, .. } => Some(call.clone()),
						_ => None,
					})
					.collect(),
			});
			for entry in executed {
				if let ContextEvent::Tool { call, result } = &entry.event {
					history.push(NativeMessage::Tool {
						tool_call_id: call.id.clone(),
						envelope: tool_envelope(entry.seq, call, result.clone()),
					});
				}
			}
		}
		markers.clear();
		for entry in after {
			single(entry, &mut history, &mut markers);
		}
		index = end;
	}
	let markers = Value::Array(markers);
	NativeContext {
		attachments: attachments(context, pinned),
		history,
		volatile: crate::context::volatile_part(
			pinned,
			markers
				.as_array()
				.is_some_and(|markers| !markers.is_empty())
				.then_some(("markers", &markers)),
		),
	}
}

/// The Model Turn each history entry belongs to, by ordinal from the oldest:
/// an `Assistant` event and the `Tool` events of its calls up to the next
/// one, grouped exactly as `native_context` renders them. Other entries
/// belong to none. Compaction prunes, retains and summarizes by these units.
pub fn model_turns(history: &[HistoryEntry]) -> Vec<Option<usize>> {
	let mut turns = Vec::with_capacity(history.len());
	let mut current: Option<(usize, &[String], Vec<&str>)> = None;
	let mut next = 0;
	for entry in history {
		match &entry.event {
			ContextEvent::Assistant { calls, .. } => {
				current = Some((next, calls, Vec::new()));
				turns.push(Some(next));
				next += 1;
			}
			ContextEvent::Tool { call, .. } => match &mut current {
				Some((turn, calls, seen))
					if calls.contains(&call.id) && !seen.contains(&call.id.as_str()) =>
				{
					seen.push(call.id.as_str());
					turns.push(Some(*turn));
				}
				_ => turns.push(None),
			},
			_ => turns.push(None),
		}
	}
	turns
}

fn tool_envelope(seq: u64, call: &ToolCall, content: Value) -> Envelope {
	Envelope::data(
		Source::Tool,
		json!({"tool": call.name, "invocation": call.id}),
		Some(seq),
		content,
	)
}

/// Render one event outside a Model Turn's calls.
fn single(entry: &HistoryEntry, history: &mut Vec<NativeMessage>, markers: &mut Vec<Value>) {
	match &entry.event {
		ContextEvent::Human {
			request,
			request_kind,
			response,
		} => history.push(NativeMessage::Human {
			request: Envelope::data(
				Source::HumanRequest,
				json!({"request_kind": request_kind}),
				Some(entry.seq),
				Value::String(request.clone()),
			),
			response: response.clone(),
		}),
		ContextEvent::ModelMediaObservation {
			text,
			through_seq,
			truncated,
		} => history.push(NativeMessage::Data {
			envelope: Envelope::data(
				Source::ModelMediaObservation,
				json!({"through_seq": through_seq}),
				Some(entry.seq),
				json!({"text": text, "truncated": truncated}),
			),
		}),
		// A result outside its Model Turn cannot be a tool message without a
		// call before it; it stays visible as enveloped data instead.
		ContextEvent::Tool { call, result } => history.push(NativeMessage::Data {
			envelope: tool_envelope(entry.seq, call, json!({"call": call, "result": result})),
		}),
		ContextEvent::RunMessageReadRequired { .. }
		| ContextEvent::RunMessageSummaryRequired { .. }
		| ContextEvent::CitationCorrectionRequired { .. } => {
			markers.push(serde_json::to_value(&entry.event).expect("serializable marker"));
		}
		// Only reached for an Assistant event without a turn, which cannot happen.
		ContextEvent::Assistant { .. } => {}
	}
}

/// Run-stable attachments in send order. An absent or empty source is left out.
fn attachments(context: &Context, pinned: &Value) -> Vec<Envelope> {
	let mut attachments = Vec::new();
	for (key, source) in
		ORDERED_STABLE_KEYS
			.iter()
			.zip([Source::Identity, Source::Task, Source::ReferenceDocuments])
	{
		let Some(value) = pinned.get(*key) else {
			continue;
		};
		let (provenance, revision) = match source {
			Source::Task => (
				json!({"task": value.get("id")}),
				value.get("revision").and_then(Value::as_u64),
			),
			_ => (json!({"binding": "run"}), None),
		};
		attachments.push(Envelope::data(source, provenance, revision, value.clone()));
	}
	if !context.run_message_summary.is_empty() {
		let through = u64::try_from(context.run_message_summary_seq).ok();
		attachments.push(Envelope::data(
			Source::RunMessageSummary,
			json!({"through_input_seq": context.run_message_summary_seq}),
			through,
			Value::String(context.run_message_summary.clone()),
		));
	}
	if let Some(summary) = context.summary_projection() {
		let through = context
			.execution_summary
			.as_ref()
			.map(|summary| summary.source.through_seq);
		attachments.push(Envelope::data(
			Source::ExecutionSummary,
			json!({"journal": "context"}),
			through,
			summary,
		));
	}
	attachments
}

impl NativeContext {
	/// Canonical chat messages after `system`: the attachment message, the
	/// history, then the volatile message with this step's media parts.
	/// `breakpoint` marks the end of the Stable Prefix, the last history
	/// message or, without history, the last attachment (ADR 0019); the
	/// volatile message is never marked.
	pub(crate) fn chat_messages(
		&self,
		media: impl Iterator<Item = Value>,
		breakpoint: bool,
	) -> Vec<Value> {
		let mut messages = Vec::with_capacity(self.history.len() + 2);
		if !self.attachments.is_empty() {
			let parts = self
				.attachments
				.iter()
				.map(|envelope| json!({"type": "text", "text": envelope.text()}))
				.collect();
			messages.push(json!({"role": "user", "content": Value::Array(parts)}));
		}
		messages.extend(self.history.iter().map(NativeMessage::chat));
		if breakpoint && let Some(last) = messages.last_mut() {
			let part = match &mut last["content"] {
				Value::Array(parts) => parts.last_mut(),
				content @ Value::String(_) => {
					*content = json!([{"type": "text", "text": content.take()}]);
					content.as_array_mut().and_then(|parts| parts.last_mut())
				}
				_ => None,
			};
			if let Some(part) = part {
				part["cache_control"] = crate::provider::cache_control();
			}
		}
		let volatile =
			Envelope::raw_text(Source::Step, json!({"scope": "request"}), &self.volatile);
		let mut media = media.peekable();
		let content = if media.peek().is_none() {
			Value::String(volatile)
		} else {
			Value::Array(
				std::iter::once(json!({"type": "text", "text": volatile}))
					.chain(media)
					.collect(),
			)
		};
		messages.push(json!({"role": "user", "content": content}));
		messages
	}
}

impl NativeMessage {
	fn chat(&self) -> Value {
		match self {
			Self::Human { request, response } => json!({"role": "user", "content": format!(
				r#"{{"in_reply_to":{},"response":{response}}}"#,
				request.text()
			)}),
			Self::Data { envelope } => json!({"role": "user", "content": envelope.text()}),
			Self::Assistant { text, tool_calls } => {
				let mut message = json!({"role": "assistant", "content": text});
				if !tool_calls.is_empty() {
					message["tool_calls"] = tool_calls
						.iter()
						.map(|call| {
							json!({"id": call.id, "type": "function", "function": {
								"name": call.name,
								"arguments": call.arguments.to_string(),
							}})
						})
						.collect();
				}
				message
			}
			Self::Tool {
				tool_call_id,
				envelope,
			} => json!({"role": "tool", "tool_call_id": tool_call_id, "content": envelope.text()}),
		}
	}
}

/// The invariant every Native request keeps: each assistant message with
/// tool calls is followed immediately by exactly one tool message per call,
/// in call order, and no tool message appears anywhere else.
pub fn check_model_turns(history: &[NativeMessage]) -> Result<()> {
	let mut index = 0;
	while index < history.len() {
		match &history[index] {
			NativeMessage::Assistant { tool_calls, .. } => {
				for (offset, call) in tool_calls.iter().enumerate() {
					match history.get(index + 1 + offset) {
						Some(NativeMessage::Tool { tool_call_id, .. })
							if *tool_call_id == call.id => {}
						_ => {
							return Err(Error::Invalid(format!(
								"Native request has a partial Model Turn at tool call {}",
								call.id
							)));
						}
					}
				}
				index += 1 + tool_calls.len();
			}
			NativeMessage::Tool { tool_call_id, .. } => {
				return Err(Error::Invalid(format!(
					"Native request has an orphan tool result {tool_call_id}"
				)));
			}
			NativeMessage::Human { .. } | NativeMessage::Data { .. } => index += 1,
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests;
