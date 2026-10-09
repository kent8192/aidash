//! Projection Versions: the frozen rules that render a Context Projection into
//! the bytes of a model request (ADR 0015). An Agent definition names one, the
//! Binding snapshot pins it, and it never changes during a Run.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

#[derive(
	Debug,
	Clone,
	Copy,
	Default,
	PartialEq,
	Eq,
	PartialOrd,
	Ord,
	Hash,
	Serialize,
	Deserialize,
	JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ProjectionVersion {
	/// The whole context as one alphabetically keyed JSON user message.
	#[default]
	Legacy,
	/// Context ordered by volatility: Run-stable context, one part per history
	/// event, then the per-step state, behind a Tenant cache salt.
	Ordered,
}

impl ProjectionVersion {
	pub fn is_legacy(&self) -> bool {
		*self == Self::Legacy
	}

	/// Versions a model definition supports when it declares none.
	pub fn legacy_only() -> Vec<Self> {
		vec![Self::Legacy]
	}

	pub fn is_legacy_only(versions: &[Self]) -> bool {
		versions == [Self::Legacy]
	}
}

/// An Agent definition's prompt-caching opt-in, pinned with the Run's Binding
/// snapshot (ADR 0019).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PromptCache {
	#[default]
	Off,
	/// Send `cache_control` breakpoints on a model that declares `explicit`.
	Explicit,
}

impl PromptCache {
	pub fn is_off(&self) -> bool {
		*self == Self::Off
	}
}

/// Pinned fields that stay fixed for a Run and therefore precede the history.
const STABLE_PINNED: [&str; 3] = ["identity", "task", "reference_documents"];
/// Run context part order. The run-message summary changes only on catch-up
/// turns and leaves the history intact; the Execution Summary changes only when
/// compaction also rewrites the history, so it comes last.
const CONTEXT_ORDER: [&str; 5] = [
	"identity",
	"task",
	"reference_documents",
	"run_message_summary",
	"summary",
];
/// Current step part order, roughly from the most to the least predictable.
/// Keys outside this list follow in sorted order.
const CURRENT_ORDER: [&str; 10] = [
	"agent_state",
	"workspace",
	"run_messages",
	"run_message_read_instruction",
	"deferred_run_message_reads",
	"deferred_workspace_read",
	"deferred_skill_read",
	"deferred_workspace_observation",
	"semantic_memory",
	"snapshot_truncated",
];

/// Split a Context Projection and the pinned snapshot into the `Ordered`
/// request context: `{"context", "history", "current"}`. Rendering into text
/// parts is [`ordered_texts`]; map order inside this value carries no meaning.
pub fn ordered_context(context: &super::Context, pinned: &Value) -> Value {
	let mut stable = Map::new();
	let mut current = Map::new();
	if let Value::Object(fields) = pinned {
		for (key, value) in fields {
			if STABLE_PINNED.contains(&key.as_str()) {
				stable.insert(key.clone(), value.clone());
			} else {
				current.insert(key.clone(), value.clone());
			}
		}
	}
	stable.insert("summary".into(), json!(context.summary));
	stable.insert(
		"run_message_summary".into(),
		json!(context.run_message_summary),
	);
	json!({
		"context": stable,
		"history": context.history,
		"current": current,
	})
}

/// The user-message text parts of an `Ordered` request, first byte to last:
/// the Run context, one `{"history":event}` part per event, then
/// `{"current":state}`. Each part ends at a content-block boundary, so earlier
/// parts stay byte-identical as the history grows.
pub fn ordered_texts(context: &Value) -> Vec<String> {
	let empty = Map::new();
	let object = |key: &str| {
		context
			.get(key)
			.and_then(Value::as_object)
			.unwrap_or(&empty)
	};
	let history = context
		.get("history")
		.and_then(Value::as_array)
		.map_or(&[][..], Vec::as_slice);
	let mut texts = Vec::with_capacity(history.len() + 2);
	texts.push(ordered_object(object("context"), &CONTEXT_ORDER));
	for event in history {
		texts.push(format!("{{\"history\":{event}}}"));
	}
	texts.push(format!(
		"{{\"current\":{}}}",
		ordered_object(object("current"), &CURRENT_ORDER)
	));
	texts
}

/// Serialize the listed keys first in the given order, then the remaining keys
/// in sorted order. Nested values keep serde_json's sorted map order.
fn ordered_object(fields: &Map<String, Value>, order: &[&str]) -> String {
	let mut text = String::from("{");
	let mut push = |key: &str, value: &Value| {
		if text.len() > 1 {
			text.push(',');
		}
		text.push_str(&Value::String(key.into()).to_string());
		text.push(':');
		text.push_str(&value.to_string());
	};
	for key in order {
		if let Some(value) = fields.get(*key) {
			push(key, value);
		}
	}
	for (key, value) in fields {
		if !order.contains(&key.as_str()) {
			push(key, value);
		}
	}
	text.push('}');
	text
}

/// HMAC-SHA256 output length used for the Tenant cache salt.
pub const CACHE_SALT_MAC_BYTES: usize = 32;

/// The first `system` line of an `Ordered` request (ADR 0016). It has a fixed
/// width, so registration can reserve its exact size without a Tenant.
pub fn cache_salt_line(key_version: u32, mac: &[u8; CACHE_SALT_MAC_BYTES]) -> String {
	let mut line = format!("Cache scope: k{key_version:08x}.");
	for byte in mac {
		line.push_str(&format!("{byte:02x}"));
	}
	line
}

/// A salt line of the same width, for request estimates made without a Tenant.
pub fn cache_salt_placeholder() -> String {
	cache_salt_line(0, &[0; CACHE_SALT_MAC_BYTES])
}

#[cfg(test)]
mod tests;
