//! Provider reasoning of an accepted response, kept as received so that every
//! later request of the Run resends it unchanged (#942).
//!
//! Entries are opaque: Aidash never reads, reorders or trims them. Each one is
//! a JSON value, so its object keys serialize in one fixed order on every
//! request; the order of the entries is the order the provider returned.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The reasoning of one accepted response, in the shape its transport returned
/// and accepts back. A Chat request sends only Chat reasoning and a Responses
/// request only Responses items; a Run never switches transport (ADR 0103).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "format", rename_all = "snake_case", deny_unknown_fields)]
pub enum Reasoning {
	/// Chat Completions `reasoning_details` entries: reasoning text, summaries,
	/// encrypted reasoning and signatures such as Gemini's.
	ChatDetails { details: Vec<Value> },
	/// Plain Chat reasoning text, kept only when a route returns no
	/// `reasoning_details`. `field` is the message key it arrived under,
	/// `reasoning` or `reasoning_content`, and the key it is sent back under.
	ChatText { field: ChatTextField, text: String },
	/// Responses `reasoning` output items, sent back before the turn's output.
	Responses { items: Vec<Value> },
}

/// The Chat message key plain reasoning text arrived under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatTextField {
	Reasoning,
	ReasoningContent,
}

impl ChatTextField {
	pub fn key(self) -> &'static str {
		match self {
			Self::Reasoning => "reasoning",
			Self::ReasoningContent => "reasoning_content",
		}
	}
}

/// The private key that carries Responses reasoning items on a rendered Chat
/// message until the Responses serialization places them as input items. The
/// Chat transport removes it.
pub const RESPONSES_ITEMS_KEY: &str = "_reasoning_items";

impl Reasoning {
	/// The reasoning of a Chat Completions message: its `reasoning_details`,
	/// or else its plain reasoning text. `None` when it has neither.
	pub fn from_chat_message(message: &Value) -> Option<Self> {
		if let Some(details) = message["reasoning_details"]
			.as_array()
			.filter(|details| !details.is_empty())
		{
			return Some(Self::ChatDetails {
				details: details.clone(),
			});
		}
		[ChatTextField::Reasoning, ChatTextField::ReasoningContent]
			.into_iter()
			.find_map(|field| {
				message[field.key()]
					.as_str()
					.filter(|text| !text.is_empty())
					.map(|text| Self::ChatText {
						field,
						text: text.to_owned(),
					})
			})
	}

	/// The `reasoning` items of a Responses output array, in order. `None`
	/// when there are none.
	pub fn from_responses_output(output: &[Value]) -> Option<Self> {
		let items = output
			.iter()
			.filter(|item| item["type"] == "reasoning")
			.cloned()
			.collect::<Vec<_>>();
		(!items.is_empty()).then_some(Self::Responses { items })
	}

	/// Add this reasoning to a rendered Chat assistant message in the form its
	/// transport resends: Chat fields directly, Responses items on
	/// [`RESPONSES_ITEMS_KEY`].
	pub fn render_chat(&self, message: &mut Value) {
		match self {
			Self::ChatDetails { details } => message["reasoning_details"] = json!(details),
			Self::ChatText { field, text } => message[field.key()] = json!(text),
			Self::Responses { items } => message[RESPONSES_ITEMS_KEY] = json!(items),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn chat_details_win_over_plain_text_and_keep_every_entry() {
		let message = json!({
			"reasoning": "plain",
			"reasoning_details": [
				{"type":"reasoning.text","text":"a","signature":null,"format":"unknown","index":0},
				{"type":"reasoning.encrypted","data":"sig","id":"call-1","format":"google-gemini-v1","index":1},
			],
		});
		let Some(Reasoning::ChatDetails { details }) = Reasoning::from_chat_message(&message)
		else {
			panic!("expected details");
		};
		assert_eq!(
			details,
			message["reasoning_details"].as_array().unwrap().clone()
		);
	}

	#[test]
	fn plain_text_keeps_the_key_it_arrived_under() {
		let message = json!({"reasoning_content": "think", "reasoning_details": []});
		let reasoning = Reasoning::from_chat_message(&message).unwrap();
		assert_eq!(
			reasoning,
			Reasoning::ChatText {
				field: ChatTextField::ReasoningContent,
				text: "think".into(),
			}
		);
		let mut rendered = json!({"role":"assistant"});
		reasoning.render_chat(&mut rendered);
		assert_eq!(
			rendered,
			json!({"role":"assistant","reasoning_content":"think"})
		);
		assert!(Reasoning::from_chat_message(&json!({"reasoning": ""})).is_none());
	}

	#[test]
	fn responses_items_keep_their_order() {
		let output = [
			json!({"type":"reasoning","id":"rs_1","encrypted_content":"x","summary":[]}),
			json!({"type":"message","content":[]}),
			json!({"type":"reasoning","id":"rs_2","encrypted_content":"y","summary":[]}),
		];
		assert_eq!(
			Reasoning::from_responses_output(&output),
			Some(Reasoning::Responses {
				items: vec![output[0].clone(), output[2].clone()],
			})
		);
		assert!(Reasoning::from_responses_output(&output[1..2]).is_none());
	}

	#[test]
	fn stored_reasoning_round_trips() {
		let reasoning = Reasoning::ChatDetails {
			details: vec![json!({"type":"reasoning.text","text":"a","index":0})],
		};
		let value = serde_json::to_value(&reasoning).unwrap();
		assert_eq!(value["format"], "chat_details");
		assert_eq!(
			serde_json::from_value::<Reasoning>(value).unwrap(),
			reasoning
		);
	}
}
