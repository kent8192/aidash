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
	/// One exact Registry-declared context candidate, always data.
	ContextSource,
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
	/// Observable provider-owned exchanges, never local tool invocations or Research evidence.
	ProviderActivity { items: Vec<Value> },
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
/// Turn. A turn without text or an executed call renders nothing.
///
/// Continuation markers move to the volatile part until a rendered Model
/// Turn answers them. A citation correction or a Completion Gate repair asks
/// for a corrected answer, so only a turn without calls answers it; tool-call
/// turns that gather the missing evidence or repair the files keep it current.
pub fn native_context(context: &Context, pinned: &Value) -> NativeContext {
	let projected = crate::context::source::project(context, pinned);
	let mut pinned = projected.snapshot;
	if !projected.step.is_empty() {
		pinned["context_sources"] = json!(projected.step);
	}
	let mut history = Vec::new();
	let mut markers = Vec::new();
	let entries = &context.history;
	let mut index = 0;
	while index < entries.len() {
		let Some(turn) = Turn::at(entries, index) else {
			single(&entries[index], &mut history, &mut markers);
			index += 1;
			continue;
		};
		if turn.rendered() {
			let results = turn
				.executed
				.iter()
				.filter_map(|&index| match &entries[index].event {
					ContextEvent::Tool { call, result } => Some((entries[index].seq, call, result)),
					_ => None,
				})
				.collect::<Vec<_>>();
			history.push(NativeMessage::Assistant {
				text: if let ContextEvent::Assistant {
					provider_activity, ..
				} = &entries[turn.start].event
				{
					if provider_activity.is_empty() {
						turn.text.map(str::to_owned)
					} else {
						Some(format!(
							"{}\nProvider tool activity (not Research evidence): {}",
							turn.text.unwrap_or_default(),
							json!(provider_activity)
						))
					}
				} else {
					turn.text.map(str::to_owned)
				},
				tool_calls: results.iter().map(|(_, call, _)| (*call).clone()).collect(),
			});
			for (seq, call, result) in results {
				history.push(NativeMessage::Tool {
					tool_call_id: call.id.clone(),
					envelope: tool_envelope(seq, call, result.clone()),
				});
			}
			if let ContextEvent::Assistant {
				provider_activity, ..
			} = &entries[turn.start].event
				&& !provider_activity.is_empty()
			{
				history.push(NativeMessage::ProviderActivity {
					items: provider_activity.clone(),
				});
			}
			markers.retain(|marker| {
				!turn.calls.is_empty()
					&& matches!(
						marker,
						ContextEvent::CitationCorrectionRequired { .. }
							| ContextEvent::CompletionRepairRequired { .. }
					)
			});
		}
		for &index in &turn.after {
			single(&entries[index], &mut history, &mut markers);
		}
		index = turn.end;
	}
	let markers = markers
		.into_iter()
		.map(|marker| serde_json::to_value(marker).expect("serializable marker"))
		.collect();
	let markers = Value::Array(markers);
	let mut attachments = attachments(context, &pinned);
	let source_position = attachments
		.iter()
		.position(|envelope| {
			matches!(
				envelope.source,
				Source::RunMessageSummary | Source::ExecutionSummary
			)
		})
		.unwrap_or(attachments.len());
	attachments.splice(source_position..source_position, projected.run);
	NativeContext {
		attachments,
		history,
		volatile: crate::context::volatile_part(
			&pinned,
			markers
				.as_array()
				.is_some_and(|markers| !markers.is_empty())
				.then_some(("markers", &markers)),
		),
	}
}

/// The Model Turn each history entry belongs to, by ordinal from the oldest:
/// an `Assistant` event and the `Tool` events of its calls up to the next
/// one, grouped exactly as `native_context` renders them. An `Assistant`
/// event that renders nothing is no turn, so it never takes a protected
/// slot. Other entries belong to none. Compaction prunes, retains and
/// summarizes by these units.
pub fn model_turns(history: &[HistoryEntry]) -> Vec<Option<usize>> {
	let mut turns = vec![None; history.len()];
	let mut next = 0;
	let mut index = 0;
	while index < history.len() {
		let Some(turn) = Turn::at(history, index) else {
			index += 1;
			continue;
		};
		if turn.rendered() {
			for &entry in std::iter::once(&turn.start).chain(&turn.executed) {
				turns[entry] = Some(next);
			}
			next += 1;
		}
		index = turn.end;
	}
	turns
}

/// One recorded Model Turn: the `Assistant` entry at `start` and every entry
/// up to `end`, the next `Assistant` entry or the end of history.
struct Turn<'a> {
	start: usize,
	end: usize,
	/// The response text, absent when blank.
	text: Option<&'a str>,
	activity: bool,
	/// Every call the response made, run or not.
	calls: &'a [String],
	/// The first `Tool` entry of each call that ran, in call order.
	executed: Vec<usize>,
	/// Every other entry of the turn, in journal order.
	after: Vec<usize>,
}

impl<'a> Turn<'a> {
	fn at(entries: &'a [HistoryEntry], start: usize) -> Option<Self> {
		let ContextEvent::Assistant {
			text,
			calls,
			provider_activity,
			..
		} = &entries[start].event
		else {
			return None;
		};
		let end = entries[start + 1..]
			.iter()
			.position(|entry| matches!(entry.event, ContextEvent::Assistant { .. }))
			.map_or(entries.len(), |offset| start + 1 + offset);
		let mut results = BTreeMap::new();
		let mut after = Vec::new();
		for (index, entry) in entries.iter().enumerate().take(end).skip(start + 1) {
			match &entry.event {
				ContextEvent::Tool { call, .. }
					if calls.contains(&call.id) && !results.contains_key(call.id.as_str()) =>
				{
					results.insert(call.id.as_str(), index);
				}
				_ => after.push(index),
			}
		}
		Some(Self {
			start,
			end,
			text: Some(text.as_str()).filter(|text| !text.trim().is_empty()),
			activity: !provider_activity.is_empty(),
			calls,
			executed: calls
				.iter()
				.filter_map(|id| results.get(id.as_str()).copied())
				.collect(),
			after,
		})
	}

	/// Whether the turn renders: text, provider-owned activity or an executed call.
	fn rendered(&self) -> bool {
		self.text.is_some() || self.activity || !self.executed.is_empty()
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
fn single<'a>(
	entry: &'a HistoryEntry,
	history: &mut Vec<NativeMessage>,
	markers: &mut Vec<&'a ContextEvent>,
) {
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
		| ContextEvent::CitationCorrectionRequired { .. }
		| ContextEvent::CompletionRepairRequired { .. } => markers.push(&entry.event),
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
		if breakpoint
			&& let Some(last) = messages
				.iter_mut()
				.rev()
				.find(|message| !message["content"].is_null())
		{
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
		let mut media = media.map(media_part).peekable();
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

/// One media part as a Native request sends it. Text parts label attachments
/// with their filename or Working Area path, which anyone who names a file
/// controls: data, enveloped. Request budgeting reserves this same framing.
pub(crate) fn media_part(part: Value) -> Value {
	match part["text"].as_str() {
		Some(text) if part["type"] == "text" => json!({"type": "text", "text": Envelope::data(
			Source::Step,
			json!({"scope": "media"}),
			None,
			Value::String(text.to_owned()),
		)
		.text()}),
		_ => part,
	}
}

impl NativeMessage {
	fn chat(&self) -> Value {
		match self {
			Self::Human { request, response } => json!({"role": "user", "content": format!(
				r#"{{"in_reply_to":{},"response":{response}}}"#,
				request.text()
			)}),
			Self::ProviderActivity { items } => {
				json!({"role":"assistant","content":null,"_provider_activity":items})
			}
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
			NativeMessage::Human { .. }
			| NativeMessage::Data { .. }
			| NativeMessage::ProviderActivity { .. } => index += 1,
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests;
