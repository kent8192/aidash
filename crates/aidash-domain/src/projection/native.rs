//! The Native Projection Version (ADR 0015, #178): a typed, provider-neutral
//! intermediate representation of one request. Recorded Model Turns become
//! assistant and tool messages instead of events inside one JSON user message.
//!
//! The harness builds this IR; `ModelRequest::input_body` gives it its
//! canonical chat serialization, which transport and request budgeting share.
use crate::context::{Context, ContextEvent, EvidenceItem, HistoryEntry, ORDERED_STABLE_KEYS};
use crate::provider::ToolCall;
use crate::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// How enveloped content may be read. `Data` never instructs; `Aidash` states
/// requirements from Aidash for this step under a `structured@*` Harness
/// Contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Trust {
	Data,
	Aidash,
}

/// Where enveloped content came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Source {
	MemoryRecall,
	Identity,
	Task,
	ReferenceDocuments,
	/// One exact Registry-declared context candidate, always data.
	ContextSource,
	RunMessageSummary,
	/// A result returned by a Subnode member.
	SubnodeResult,
	ExecutionSummary,
	Tool,
	ModelMediaObservation,
	/// The question a Human answer replies to. It may quote model or tool
	/// output, so only the answer itself is unwrapped.
	HumanRequest,
	/// This step's volatile content.
	Step,
	/// The Harness requirements of this step under a `structured@*` contract.
	Harness,
	/// Evidence a step admitted into history (#942).
	Evidence,
}

/// The provenance envelope around every piece of enveloped content in a Native
/// request. Data-derived content is wrapped with trust `data`; under a
/// `structured@*` Harness Contract the Harness requirements are wrapped with
/// trust `aidash` instead. Human input is never wrapped. Field order is the
/// serialized order.
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

	pub fn subnode_result(sender: &str, seq: i64, content: Value) -> Self {
		Self::data(
			Source::SubnodeResult,
			json!({"sender":sender}),
			u64::try_from(seq).ok(),
			content,
		)
	}

	/// Canonical text of the envelope: typed field order, sorted JSON maps.
	pub fn text(&self) -> String {
		serde_json::to_string(self).expect("serializable provenance envelope")
	}

	/// The Harness requirements envelope of a structured Harness Contract.
	fn harness(content: Value) -> Self {
		Self {
			source: Source::Harness,
			provenance: json!("aidash"),
			revision: None,
			trust: Trust::Aidash,
			content,
		}
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
	/// A Human answer, sent as an unwrapped user message. It is the only
	/// instruction channel outside `system` and the Harness envelope; the
	/// request it replies to is enveloped.
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
	/// A step's bookkeeping (#942), sent as a user message at the position it
	/// was recorded: the state as data, then the Harness requirements of a
	/// `structured@*` contract with trust `aidash`.
	StepState {
		state: Envelope,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		harness: Option<Envelope>,
	},
}

/// A complete Native request context: Run-stable attachments, then history
/// rendered as whole Model Turns and recorded step states, then this step's
/// volatile evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeContext {
	/// Run-stable sources, one envelope each, sent in one user message right
	/// after `system`. They are part of the Stable Prefix.
	pub attachments: Vec<Envelope>,
	/// History, ending with this step's state when it differs from the last
	/// recorded one and is not recorded yet ([`record_step_state`]); it
	/// renders byte for byte as it will once recorded.
	pub history: Vec<NativeMessage>,
	/// Canonical JSON of this step's volatile evidence, sent enveloped as the
	/// trailing user message. It is never stored in history; an empty object
	/// without media sends no message.
	pub volatile: String,
	/// A Harness requirements envelope text sent after the volatile part.
	/// [`native_context`] never sets it: since #942 the requirements travel
	/// in the step state.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub harness: Option<String>,
}

/// The reserved pinned key that carries the Harness requirements envelope
/// content of a `structured@*` contract. It is never part of the step's data
/// envelope; the Harness builds it and the step state carries it.
pub const HARNESS_REQUIREMENTS: &str = "harness_requirements";

/// Pinned keys of a Native step's bookkeeping (#942): state and instructions
/// that carry no evidence. They go to the step state, recorded in history
/// only when it changes. Every other per-step key is evidence and stays in
/// the volatile part.
pub const STEP_STATE_KEYS: [&str; 8] = [
	"agent_state",
	"turn_instructions",
	"run_message_read_instruction",
	"deferred_run_message_reads",
	"deferred_skill_read",
	"deferred_workspace_observation",
	"deferred_workspace_read",
	"journal_read_notice",
];

/// Pinned keys whose evidence a Native step appends to history once, item by
/// item (#942). Semantic memory stays in the volatile part: its content is
/// never journaled, and its per-step recheck fails the step instead of
/// marking a revoked hit.
pub const EVIDENCE_KEYS: [&str; 2] = ["workspace", "run_messages"];

/// Evidence keys admitted whatever the quota: the Run bounds its run-directed
/// input when it accepts it, and its required reads depend on every entry.
const MANDATORY_EVIDENCE: [&str; 1] = ["run_messages"];

/// Fields of an evidence key that describe this step's page rather than any
/// entry; they change with every count and stay in the volatile part.
const PAGE_FIELDS: [&str; 1] = ["pages"];

/// The evidence items of `pinned`, in send order: every entry of a list, and
/// every entry of each list field or each other field of an object.
fn evidence_items(pinned: &Value) -> Vec<EvidenceItem> {
	let item = |key: &str, field: Option<&str>, listed: bool, content: &Value| {
		EvidenceItem::new(key, field, listed, content.clone())
	};
	let mut items = Vec::new();
	for key in EVIDENCE_KEYS {
		match pinned.get(key) {
			Some(Value::Array(entries)) => {
				items.extend(entries.iter().map(|entry| item(key, None, true, entry)));
			}
			Some(Value::Object(fields)) => {
				for (field, value) in fields {
					if PAGE_FIELDS.contains(&field.as_str()) {
						continue;
					}
					match value {
						Value::Array(entries) => items.extend(
							entries
								.iter()
								.map(|entry| item(key, Some(field), true, entry)),
						),
						value => items.push(item(key, Some(field), false, value)),
					}
				}
			}
			Some(Value::Null) | None => {}
			Some(value) => items.push(item(key, None, false, value)),
		}
	}
	items
}

/// Digests of every evidence item history holds, offloaded or not.
fn recorded_evidence(context: &Context) -> BTreeSet<&str> {
	context
		.events()
		.filter_map(|event| match event {
			ContextEvent::Evidence { items } => Some(items),
			_ => None,
		})
		.flatten()
		.map(|item| item.digest.as_str())
		.collect()
}

/// The evidence of `pinned` that history does not hold yet.
fn pending_evidence(context: &Context, pinned: &Value) -> Vec<EvidenceItem> {
	let recorded = recorded_evidence(context);
	let mut seen = BTreeSet::new();
	evidence_items(pinned)
		.into_iter()
		.filter(|item| !recorded.contains(item.digest.as_str()) && seen.insert(item.digest.clone()))
		.collect()
}

/// Admit this step's new evidence into history (#942), in send order, while
/// the encoded items stay within `quota` bytes; Run messages are always
/// admitted. Admission is final: an
/// admitted item is never trimmed later, and a refused one is removed from
/// `pinned`, so this request never carries it; a later step may admit it.
/// Returns how many items it refused. Only Native Runs record evidence.
pub fn record_evidence(context: &mut Context, pinned: &mut Value, quota: usize) -> usize {
	let mut admitted = Vec::new();
	let mut refused = BTreeSet::new();
	let mut used = 0_usize;
	for item in pending_evidence(context, pinned) {
		let size = item.content.to_string().len();
		if MANDATORY_EVIDENCE.contains(&item.key.as_str()) || used.saturating_add(size) <= quota {
			used += size;
			admitted.push(item);
		} else {
			refused.insert(item.digest);
		}
	}
	if !refused.is_empty() {
		for key in EVIDENCE_KEYS {
			let Some(value) = pinned.get_mut(key) else {
				continue;
			};
			let keep = |field: Option<&str>, entry: &Value| {
				!refused.contains(&EvidenceItem::digest(key, field, entry))
			};
			match value {
				Value::Array(entries) => entries.retain(|entry| keep(None, entry)),
				Value::Object(fields) => fields.retain(|field, value| match value {
					Value::Array(entries) => {
						entries.retain(|entry| keep(Some(field), entry));
						true
					}
					value => PAGE_FIELDS.contains(&field.as_str()) || keep(Some(field), value),
				}),
				_ => {}
			}
		}
	}
	if !admitted.is_empty() {
		context.push(ContextEvent::Evidence { items: admitted });
	}
	refused.len()
}

/// The value of evidence `key` as this request delivers it: `pinned`'s value
/// with every item whose content history no longer holds as admitted (it
/// was offloaded) set to `null`. Source
/// delivery receipts read it, so offloaded bytes never earn one.
pub fn delivered_evidence(context: &Context, pinned: &Value, key: &str) -> Option<Value> {
	let mut value = pinned.get(key)?.clone();
	let offloaded: BTreeSet<&str> = context
		.events()
		.filter_map(|event| match event {
			ContextEvent::Evidence { items } => Some(items),
			_ => None,
		})
		.flatten()
		.filter(|item| {
			item.digest != EvidenceItem::digest(&item.key, item.field.as_deref(), &item.content)
		})
		.map(|item| item.digest.as_str())
		.collect();
	let gone = |field: Option<&str>, entry: &Value| {
		offloaded.contains(EvidenceItem::digest(key, field, entry).as_str())
	};
	match &mut value {
		Value::Array(entries) => {
			for entry in entries.iter_mut().filter(|entry| gone(None, entry)) {
				*entry = Value::Null;
			}
		}
		Value::Object(fields) => {
			for (field, value) in fields.iter_mut() {
				match value {
					Value::Array(entries) => {
						for entry in entries.iter_mut() {
							if gone(Some(field), entry) {
								*entry = Value::Null;
							}
						}
					}
					value => {
						if gone(Some(field), value) {
							*value = Value::Null;
						}
					}
				}
			}
		}
		_ => {}
	}
	Some(value)
}

/// How admitted evidence renders: one data envelope whose content groups the
/// items by key and field in admission order. Its revision stays `null`, so
/// the pending and the recorded form are the same bytes.
fn evidence_message(items: &[EvidenceItem]) -> NativeMessage {
	let mut content = serde_json::Map::new();
	for item in items {
		let value = item.content.clone();
		let slot = content
			.entry(item.key.clone())
			.or_insert_with(|| match item.field {
				Some(_) => json!({}),
				None => json!([]),
			});
		let target = match (&item.field, slot) {
			(Some(field), Value::Object(fields)) => {
				fields.entry(field.clone()).or_insert(Value::Null)
			}
			(_, slot) => slot,
		};
		match target {
			Value::Array(entries) if item.listed => entries.push(value),
			Value::Null if item.listed => *target = json!([value]),
			// Items of one key are either listed or whole fields; a whole
			// field replaces nothing else.
			target => *target = value,
		}
	}
	NativeMessage::Data {
		envelope: Envelope::data(
			Source::Evidence,
			json!({"scope": "step"}),
			None,
			Value::Object(content),
		),
	}
}

/// The notice a Native step's `agent_state` carries instead of a step
/// counter (#942), which would change the state, and so the request, every
/// step. It changes once at 80 % and once at 90 % of `max_steps`.
pub fn step_limit_notice(step: i32, max_steps: i32) -> Option<&'static str> {
	let used = i64::from(step) * 10;
	let max = i64::from(max_steps);
	if max <= 0 {
		None
	} else if used >= max * 9 {
		Some("90% of this Run's steps are used. Finish the task in the next few steps.")
	} else if used >= max * 8 {
		Some("80% of this Run's steps are used. Plan to finish within the remaining steps.")
	} else {
		None
	}
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
/// Continuation markers move to the step state, or to its Harness
/// requirements under a `structured@*` contract, until a rendered Model Turn
/// answers them. A citation correction or a Completion Gate repair asks for a
/// corrected answer, so only a turn without calls answers it; tool-call turns
/// that gather the missing evidence or repair the files keep it current. The
/// notice of Unassigned Tasks is kept the same way: delegating one of two
/// listed Tasks must not hide the warning about the other.
///
/// Recorded step states render where they were recorded. This step's state
/// ends the history when it differs from the last recorded one, exactly as
/// [`record_step_state`] will record it, so a request built before recording
/// measures and sends the same bytes.
pub fn native_context(context: &Context, pinned: &Value) -> NativeContext {
	let projected = crate::context::source::project(context, pinned);
	let mut pinned = projected.snapshot;
	if !projected.step.is_empty() {
		pinned["context_sources"] = json!(projected.step);
	}
	let (mut history, markers) = render_history(&context.history);
	if let Some(state) = pending_step_state(context, &pinned, &markers) {
		history.push(step_message(&state).expect("a step state event"));
	}
	let evidence = pending_evidence(context, &pinned);
	if !evidence.is_empty() {
		history.push(evidence_message(&evidence));
	}
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
	if let Some(fields) = pinned.as_object_mut() {
		for key in STEP_STATE_KEYS.into_iter().chain([HARNESS_REQUIREMENTS]) {
			fields.remove(key);
		}
		// Only an evidence key's page fields stay per step.
		for key in EVIDENCE_KEYS {
			let pages = match fields.remove(key) {
				Some(Value::Object(object)) => object
					.into_iter()
					.filter(|(field, _)| PAGE_FIELDS.contains(&field.as_str()))
					.collect::<serde_json::Map<_, _>>(),
				_ => continue,
			};
			if !pages.is_empty() {
				fields.insert(key.into(), Value::Object(pages));
			}
		}
	}
	NativeContext {
		attachments,
		history,
		volatile: crate::context::volatile_part(&pinned, None),
		harness: None,
	}
}

/// This step's state when it differs from the last one recorded in history.
fn pending_step_state(
	context: &Context,
	pinned: &Value,
	markers: &[&ContextEvent],
) -> Option<ContextEvent> {
	let state = step_state(pinned, markers)?;
	let last = context
		.events()
		.rev()
		.find(|event| matches!(event, ContextEvent::StepState { .. }));
	(last != Some(&state)).then_some(state)
}

/// Record this step's state in history when it differs from the last one,
/// so it is journaled and later requests send it at the same position.
/// Returns whether it recorded one. Only Native Runs record step states.
pub fn record_step_state(context: &mut Context, pinned: &Value) -> bool {
	let (_, markers) = render_history(&context.history);
	let Some(state) = pending_step_state(context, pinned, &markers) else {
		return false;
	};
	context.push(state);
	true
}

/// The step state of `pinned` with the open `markers`, or `None` when it has
/// nothing to say.
fn step_state(pinned: &Value, markers: &[&ContextEvent]) -> Option<ContextEvent> {
	let markers = markers
		.iter()
		.map(|marker| serde_json::to_value(marker).expect("serializable marker"))
		.collect::<Vec<_>>();
	let mut state = serde_json::Map::new();
	for key in STEP_STATE_KEYS {
		if let Some(value) = pinned.get(key) {
			state.insert(key.into(), value.clone());
		}
	}
	// A structured Harness Contract builds one requirements envelope; markers
	// join it because only the projected history knows which are still open.
	let harness = match pinned.get(HARNESS_REQUIREMENTS) {
		Some(Value::Object(content)) => {
			let mut content = content.clone();
			if !markers.is_empty() {
				content.insert("markers".into(), Value::Array(markers));
			}
			(!content.is_empty()).then_some(Value::Object(content))
		}
		_ => {
			if !markers.is_empty() {
				state.insert("markers".into(), Value::Array(markers));
			}
			None
		}
	};
	(!state.is_empty() || harness.is_some()).then(|| ContextEvent::StepState {
		state: Value::Object(state),
		harness,
	})
}

/// How a step state renders: its revision stays `null`, so the pending and
/// the recorded form are the same bytes.
fn step_message(event: &ContextEvent) -> Option<NativeMessage> {
	let ContextEvent::StepState { state, harness } = event else {
		return None;
	};
	Some(NativeMessage::StepState {
		state: Envelope::data(Source::Step, json!({"scope": "step"}), None, state.clone()),
		harness: harness.clone().map(Envelope::harness),
	})
}

/// Render recorded history and collect the continuation markers no rendered
/// Model Turn has answered yet.
fn render_history(entries: &[HistoryEntry]) -> (Vec<NativeMessage>, Vec<&ContextEvent>) {
	let mut history = Vec::new();
	let mut markers = Vec::new();
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
							| ContextEvent::UnassignedTasksTold { .. }
					)
			});
		}
		for &index in &turn.after {
			single(&entries[index], &mut history, &mut markers);
		}
		index = turn.end;
	}
	(history, markers)
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
		ContextEvent::MemoryRecall { content, .. } => history.push(NativeMessage::Data {
			envelope: Envelope::data(
				Source::MemoryRecall,
				json!({}),
				Some(entry.seq),
				content
					.clone()
					.unwrap_or_else(|| json!({"status":"unavailable"})),
			),
		}),
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
		| ContextEvent::UnassignedTasksTold { .. }
		| ContextEvent::CompletionRepairRequired { .. } => markers.push(&entry.event),
		ContextEvent::StepState { .. } => {
			history.extend(step_message(&entry.event));
		}
		ContextEvent::Evidence { items } => history.push(evidence_message(items)),
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
	/// history, then the volatile message with this step's media parts and,
	/// last, any Harness requirements envelope. Without volatile evidence,
	/// media or requirements there is no volatile message.
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
		let mut media = media.map(media_part).peekable();
		if self.volatile == "{}" && self.harness.is_none() && media.peek().is_none() {
			return messages;
		}
		let volatile =
			Envelope::raw_text(Source::Step, json!({"scope": "request"}), &self.volatile);
		let content = if self.harness.is_none() && media.peek().is_none() {
			Value::String(volatile)
		} else {
			let mut parts = vec![json!({"type": "text", "text": volatile})];
			parts.extend(media);
			// The Harness requirements are the last part, after this step's
			// media, so a media-bearing request keeps every label intact.
			if let Some(harness) = &self.harness {
				parts.push(json!({"type": "text", "text": harness}));
			}
			Value::Array(parts)
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
			Self::StepState {
				state,
				harness: None,
			} => json!({"role": "user", "content": state.text()}),
			Self::StepState {
				state,
				harness: Some(harness),
			} => json!({"role": "user", "content": [
				{"type": "text", "text": state.text()},
				{"type": "text", "text": harness.text()},
			]}),
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
			| NativeMessage::StepState { .. }
			| NativeMessage::ProviderActivity { .. } => index += 1,
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests;
