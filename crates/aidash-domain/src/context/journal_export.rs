//! Serializable, data-only view of resolved Context Journal events.
use super::HistoryEntry;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One journal event. `details` preserves fields other than the common ones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalLine {
	pub seq: u64,
	pub kind: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub text: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub call: Option<Value>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub result: Option<Value>,
	#[serde(flatten)]
	pub details: serde_json::Map<String, Value>,
}

impl From<&HistoryEntry> for JournalLine {
	fn from(entry: &HistoryEntry) -> Self {
		let Value::Object(mut fields) =
			serde_json::to_value(&entry.event).expect("ContextEvent serializes to an object")
		else {
			unreachable!()
		};
		let kind = fields
			.remove("kind")
			.and_then(|v| v.as_str().map(str::to_owned))
			.expect("ContextEvent has a kind");
		let text = fields
			.remove("text")
			.and_then(|v| v.as_str().map(str::to_owned));
		// Recorded provider reasoning is resent to the Run's own model only
		// (#942). Journal Query, search, reads and memory extraction never see
		// it, so it is not part of the export.
		fields.remove("reasoning");
		let result = fields.remove("result");
		let mut call = fields.remove("call");
		if result
			.as_ref()
			.is_some_and(super::ContextEvent::is_revoked_result)
			&& let Some(call) = call.as_mut()
		{
			call["arguments"] = super::ContextEvent::revoked_arguments();
		}
		Self {
			seq: entry.seq,
			kind,
			text,
			call,
			result,
			details: fields,
		}
	}
}

/// Encode one event per line, including a trailing newline for nonempty exports.
pub fn to_jsonl(lines: &[JournalLine]) -> String {
	let mut output = String::new();
	for line in lines {
		output.push_str(&serde_json::to_string(line).expect("JournalLine is JSON serializable"));
		output.push('\n');
	}
	output
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::context::{ContextEvent, HistoryEntry};
	use serde_json::json;

	#[test]
	fn jsonl_keeps_sequences_and_event_fields_without_breaking_lines() {
		let entries = [
			HistoryEntry::new(
				4,
				ContextEvent::Human {
					request: "question".into(),
					request_kind: "text".into(),
					response: json!("a\nb"),
				},
			),
			HistoryEntry::new(
				8,
				ContextEvent::Assistant {
					provider_activity: vec![],
					epoch: 2,
					text: "answer".into(),
					calls: vec![],
					reasoning: Some(crate::provider::reasoning::Reasoning::ChatDetails {
						details: vec![json!({"type":"reasoning.text","text":"private"})],
					}),
				},
			),
		];
		let lines: Vec<_> = entries.iter().map(JournalLine::from).collect();
		let output = to_jsonl(&lines);
		let parsed: Vec<Value> = output
			.lines()
			.map(|line| serde_json::from_str(line).unwrap())
			.collect();
		assert_eq!(
			parsed,
			vec![
				json!({"seq":4,"kind":"human","request":"question","request_kind":"text","response":"a\nb"}),
				json!({"seq":8,"kind":"assistant","epoch":2,"text":"answer","calls":[]})
			]
		);
		assert!(!output.contains("private"), "reasoning is not exported");
	}

	#[test]
	fn revoked_research_hides_call_arguments_but_expiry_keeps_them() {
		let call = crate::provider::ToolCall {
			id: "research-call".into(),
			name: "research_fetch".into(),
			arguments: json!({"url":"https://private.example/"}),
		};
		let revoked = JournalLine::from(&HistoryEntry::new(
			1,
			ContextEvent::tool(call.clone(), ContextEvent::revoked_result(&call.id)),
		));
		assert_eq!(
			revoked.call.unwrap()["arguments"],
			ContextEvent::revoked_arguments()
		);
		let expired = JournalLine::from(&HistoryEntry::new(
			2,
			ContextEvent::tool(call.clone(), ContextEvent::expired_result(&call.id)),
		));
		assert_eq!(expired.call.unwrap()["arguments"], call.arguments);
	}
}
