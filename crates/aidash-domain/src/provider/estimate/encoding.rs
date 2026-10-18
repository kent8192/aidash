//! Token Encodings (ADR 0017, revised 2026-10-17): a closed, code-defined list
//! of rules that turn text into the tokens of one model family. A model
//! definition names one explicitly; it is never inferred from the model slug.
//!
//! Counting covers only the text the model sees. Chat templates and tool
//! rendering add tokens no encoding can see, so a count is still an upper
//! bound only together with the per-message and per-tool framing constants.
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

#[cfg(feature = "token-encodings")]
mod engine;
#[cfg(test)]
mod tests;

/// Tokens added for each chat message: role markers and template separators.
pub const MESSAGE_TOKENS: usize = 8;
/// Tokens added for each tool definition, whose schema the provider renders
/// in its own format rather than as the JSON that is counted.
pub const TOOL_TOKENS: usize = 32;
/// Longest piece of text counted in one pass. Longer strings are counted in
/// pieces split at whitespace, which bounds the time adversarial text can
/// take and lets unchanged pieces reuse their earlier count.
pub const CHUNK_BYTES: usize = 16 * 1024;
/// How far back from a chunk's end a split looks for whitespace.
#[cfg(any(feature = "token-encodings", test))]
const SPLIT_LOOKBACK: usize = 1024;

/// One model family's tokenizer, embedded in the server binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TokenEncoding {
	/// OpenAI's `o200k_base`, used from GPT-4o through GPT-5. Counted with the
	/// gpt-oss tokenizer, which only adds special tokens to it.
	O200kBase,
	/// Gemma 4, which Google's SDK uses to count Gemini 3.1 and later.
	Gemma4,
}

impl TokenEncoding {
	pub const ALL: [Self; 2] = [Self::O200kBase, Self::Gemma4];

	pub fn name(self) -> &'static str {
		match self {
			Self::O200kBase => "o200k_base",
			Self::Gemma4 => "gemma4",
		}
	}

	pub fn from_name(name: &str) -> Option<Self> {
		Self::ALL
			.into_iter()
			.find(|encoding| encoding.name() == name)
	}

	/// The version of the embedded file. Replacing a file is a new version, so
	/// a step pinned to the old one resumes on bytes rather than on a count
	/// it never relied on.
	pub const fn version(self) -> u32 {
		match self {
			Self::O200kBase | Self::Gemma4 => 1,
		}
	}

	/// Whether this binary can count with the encoding. A build without the
	/// `token-encodings` feature counts nothing and estimates in bytes.
	pub fn available(self) -> bool {
		#[cfg(feature = "token-encodings")]
		{
			engine::Engine::load(self).is_some()
		}
		#[cfg(not(feature = "token-encodings"))]
		{
			false
		}
	}

	/// Tokens of `text`. `None` when the encoding is unavailable.
	pub fn count(self, text: &str) -> Option<usize> {
		#[cfg(feature = "token-encodings")]
		{
			engine::Engine::load(self).map(|engine| engine.count(text))
		}
		#[cfg(not(feature = "token-encodings"))]
		{
			let _ = text;
			None
		}
	}

	/// Tokens of the model-visible text in a provider request body, plus the
	/// message and tool framing constants. Every string in a message counts
	/// as the text the model receives, its JSON escaping decoded once. Tool
	/// definitions and any other top-level value count as their JSON.
	pub fn count_body(self, body: &Value) -> Option<usize> {
		if !self.available() {
			return None;
		}
		let Value::Object(fields) = body else {
			return self.count(&body.to_string());
		};
		let mut tokens = 0_usize;
		for (key, value) in fields {
			let counted = match (key.as_str(), value) {
				("messages", Value::Array(messages)) => {
					messages.iter().try_fold(0_usize, |sum, message| {
						Some(
							sum.saturating_add(MESSAGE_TOKENS)
								.saturating_add(self.count_strings(message)?),
						)
					})?
				}
				("tools", Value::Array(tools)) => tools.iter().try_fold(0_usize, |sum, tool| {
					Some(
						sum.saturating_add(TOOL_TOKENS)
							.saturating_add(self.count(&tool.to_string())?),
					)
				})?,
				_ => self.count(&value.to_string())?,
			};
			tokens = tokens.saturating_add(counted);
		}
		Some(tokens)
	}

	fn count_strings(self, value: &Value) -> Option<usize> {
		match value {
			Value::String(text) => self.count(text),
			Value::Array(items) => items.iter().try_fold(0_usize, |sum, item| {
				Some(sum.saturating_add(self.count_strings(item)?))
			}),
			Value::Object(fields) => fields.values().try_fold(0_usize, |sum, item| {
				Some(sum.saturating_add(self.count_strings(item)?))
			}),
			Value::Null => Some(0),
			Value::Bool(_) | Value::Number(_) => self.count(&value.to_string()),
		}
	}
}

/// A pinned snapshot names its encoding leniently: a name this binary no
/// longer lists loads as `None`, so the Run still loads and its step resumes
/// on bytes instead of failing to decode.
pub(super) fn lenient<'de, D: Deserializer<'de>>(
	deserializer: D,
) -> Result<Option<TokenEncoding>, D::Error> {
	Ok(Option::<String>::deserialize(deserializer)?
		.as_deref()
		.and_then(TokenEncoding::from_name))
}

/// `text` split into pieces of at most `size` bytes. A split prefers the
/// start of a whitespace run within the last `SPLIT_LOOKBACK` bytes, where
/// encodings start a new word anyway, and otherwise falls on a character
/// boundary.
#[cfg(any(feature = "token-encodings", test))]
pub(crate) fn chunks(text: &str, size: usize) -> impl Iterator<Item = &str> {
	let size = size.max(4);
	let mut rest = text;
	std::iter::from_fn(move || {
		if rest.is_empty() {
			return None;
		}
		if rest.len() <= size {
			return Some(std::mem::take(&mut rest));
		}
		let end = rest.floor_char_boundary(size);
		let window = rest.floor_char_boundary(end.saturating_sub(SPLIT_LOOKBACK));
		let mut previous_whitespace = true;
		let mut split = None;
		for (index, character) in rest[window..end].char_indices() {
			let whitespace = character.is_whitespace();
			if whitespace && !previous_whitespace {
				split = Some(window + index);
			}
			previous_whitespace = whitespace;
		}
		let split = split.filter(|split| *split > 0).unwrap_or(end);
		let (chunk, tail) = rest.split_at(split);
		rest = tail;
		Some(chunk)
	})
}
