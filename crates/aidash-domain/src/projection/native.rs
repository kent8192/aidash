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
}

/// One history message of a Native request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeMessage {
	/// A Human answer, sent as a user message without an envelope.
	Human {
		request_kind: String,
		request: String,
		response: Value,
	},
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
	/// Canonical JSON of this step's volatile content, sent as the trailing
	/// user message. It is never stored in history.
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
			request_kind: request_kind.clone(),
			request: request.clone(),
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
		| ContextEvent::RunMessageSummaryRequired { .. } => {
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
