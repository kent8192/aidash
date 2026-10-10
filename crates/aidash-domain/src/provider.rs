//! Inference inputs, outputs, media validation, and request budgets.
use crate::{Error, Result};
use base64::Engine;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolSpec {
	pub name: String,
	pub description: String,
	pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolCall {
	pub id: String,
	pub name: String,
	#[serde(deserialize_with = "crate::entities::required_json")]
	pub arguments: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ModelRequest {
	pub instructions: String,
	pub context: ModelContext,
	pub tools: Vec<ToolSpec>,
	pub max_output_tokens: u32,
	/// Resolved, authorized input for this inference only. Never persist bytes
	/// in the durable context or serialize them with the request metadata.
	#[serde(skip)]
	pub content_parts: Vec<ContentPart>,
	/// Set only for salted Projection Versions. Omitted otherwise, so Legacy
	/// request metadata and its inference digest stay byte-identical.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub cache_scope: Option<crate::projection::CacheScope>,
	/// Mark the end of `system` and of the Ordered Stable Prefix part with
	/// `cache_control` (ADR 0019). The volatile part is never marked. Valid for
	/// Ordered requests only; omitted when false, so other request metadata and
	/// inference digests keep their bytes.
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub cache_breakpoints: bool,
}

/// Model-visible context in the shape fixed by the Run's Projection Version.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ModelContext {
	/// Ordered: two text parts, the Stable Prefix part first.
	Ordered(OrderedContext),
	/// Legacy: one JSON value sent as a single text message.
	Legacy(Value),
}

impl Default for ModelContext {
	fn default() -> Self {
		Self::Legacy(Value::Null)
	}
}

impl From<Value> for ModelContext {
	fn from(value: Value) -> Self {
		Self::Legacy(value)
	}
}

impl ModelContext {
	pub fn legacy_mut(&mut self) -> Option<&mut Value> {
		match self {
			Self::Legacy(value) => Some(value),
			Self::Ordered(_) => None,
		}
	}

	/// Leading text parts of the user message, in send order.
	fn text_parts(&self) -> Vec<std::borrow::Cow<'_, str>> {
		match self {
			Self::Legacy(value) => vec![value.to_string().into()],
			Self::Ordered(ordered) => vec![
				ordered.stable.as_str().into(),
				ordered.volatile.as_str().into(),
			],
		}
	}
}

/// Pre-rendered Ordered parts. Each is canonical JSON whose key order was fixed
/// by typed serialization, never by a `Value` map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrderedContext {
	/// Run-stable context, summaries and history: the end of the Stable Prefix.
	pub stable: String,
	/// Per-step content that is never part of the Stable Prefix.
	pub volatile: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelResponse {
	pub text: String,
	pub tool_calls: Vec<ToolCall>,
	pub input_tokens: u64,
	pub output_tokens: u64,
	pub usage_complete: bool,
}

#[derive(Debug, Clone)]
pub enum ContentPart {
	Text(String),
	Image { media_type: String, bytes: Vec<u8> },
	Audio { format: String, bytes: Vec<u8> },
}

impl ContentPart {
	pub fn format_for_media_type(media_type: &str) -> Result<&'static str> {
		match media_type {
			"image/png" => Ok("image/png"),
			"image/jpeg" => Ok("image/jpeg"),
			"image/gif" => Ok("image/gif"),
			"image/webp" => Ok("image/webp"),
			"audio/wav" | "audio/x-wav" => Ok("wav"),
			"audio/mpeg" => Ok("mp3"),
			"audio/mp4" | "audio/x-m4a" => Ok("m4a"),
			"audio/aac" => Ok("aac"),
			"audio/ogg" => Ok("ogg"),
			"audio/webm" => Ok("webm"),
			"audio/flac" | "audio/x-flac" => Ok("flac"),
			_ => Err(Error::Invalid(format!(
				"unsupported model media type: {media_type}"
			))),
		}
	}

	pub fn modality_for_media_type(media_type: &str) -> Result<&'static str> {
		match media_type {
			"image/png" | "image/jpeg" | "image/gif" | "image/webp" => Ok("image"),
			"audio/wav" | "audio/x-wav" | "audio/mpeg" | "audio/mp4" | "audio/x-m4a"
			| "audio/aac" | "audio/ogg" | "audio/webm" | "audio/flac" | "audio/x-flac" => Ok("audio"),
			_ => Err(Error::Invalid(format!(
				"unsupported model media type: {media_type}"
			))),
		}
	}

	pub fn from_media(media_type: &str, bytes: Vec<u8>) -> Result<Self> {
		let part = match Self::modality_for_media_type(media_type)? {
			"image" => Self::Image {
				media_type: media_type.into(),
				bytes,
			},
			_ => Self::Audio {
				format: Self::format_for_media_type(media_type)?.into(),
				bytes,
			},
		};
		part.validate()?;
		Ok(part)
	}

	fn openrouter(&self) -> Value {
		match self {
			Self::Text(text) => json!({"type":"text","text":text}),
			Self::Image { media_type, bytes } => {
				json!({"type":"image_url","image_url":{"url":format!("data:{media_type};base64,{}",base64::engine::general_purpose::STANDARD.encode(bytes))}})
			}
			Self::Audio { format, bytes } => {
				json!({"type":"input_audio","input_audio":{"data":base64::engine::general_purpose::STANDARD.encode(bytes),"format":format}})
			}
		}
	}

	/// Transport framing of this part with any encoded media payload left
	/// empty, so estimates keep the array shape without counting base64 bytes.
	fn estimate_frame(&self) -> Value {
		match self {
			Self::Text(_) => self.openrouter(),
			Self::Image { media_type, .. } => {
				json!({"type":"image_url","image_url":{"url":format!("data:{media_type};base64,")}})
			}
			Self::Audio { format, .. } => {
				json!({"type":"input_audio","input_audio":{"data":"","format":format}})
			}
		}
	}

	fn validate(&self) -> Result<()> {
		match self {
			Self::Text(_) => Ok(()),
			Self::Image { media_type, bytes } => {
				let valid = match media_type.as_str() {
					"image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
					"image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
					"image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
					"image/webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
					_ => false,
				};
				if !valid {
					return Err(Error::Invalid(
						"image MIME does not match a supported file signature".into(),
					));
				}
				Ok(())
			}
			Self::Audio { format, bytes } => {
				let valid = match format.as_str() {
					"wav" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE"),
					"mp3" => {
						bytes.starts_with(b"ID3")
							|| bytes.get(..4).is_some_and(|header| {
								header[0] == 0xff
							&& header[1] & 0xe0 == 0xe0 // 11-bit sync word
							&& (header[1] >> 3) & 0x03 != 0x01 // reserved version
							&& (header[1] >> 1) & 0x03 == 0x01 // Layer III
							&& header[2] >> 4 != 0x0f // reserved bitrate
							&& (header[2] >> 2) & 0x03 != 0x03 // reserved sample rate
							})
					}
					"m4a" => bytes.get(4..8) == Some(b"ftyp"),
					"aac" => bytes
						.get(..2)
						.is_some_and(|header| header[0] == 0xff && header[1] & 0xf6 == 0xf0),
					"ogg" => bytes.starts_with(b"OggS"),
					"webm" => bytes.starts_with(b"\x1a\x45\xdf\xa3"),
					"flac" => bytes.starts_with(b"fLaC"),
					_ => false,
				};
				if !valid {
					return Err(Error::Invalid(
						"audio format does not match a supported file signature".into(),
					));
				}
				Ok(())
			}
		}
	}
}

/// A five-minute provider cache breakpoint, the provider default (ADR 0019).
fn cache_control() -> Value {
	json!({"type":"ephemeral"})
}

impl ModelRequest {
	fn media_tokens(parts: &[ContentPart]) -> usize {
		parts.iter().fold(0_usize, |total, part| {
			total.saturating_add(match part {
				ContentPart::Text(_) => 0,
				ContentPart::Image { bytes, .. } => {
					4096_usize.saturating_add(bytes.len().div_ceil(256))
				}
				ContentPart::Audio { bytes, .. } => {
					1024_usize.saturating_add(bytes.len().div_ceil(16))
				}
			})
		})
	}

	/// Space added to a media-free request by these parts, including text
	/// labels, media part framing, and the conservative provider-side media
	/// token allowance.
	pub fn content_parts_reservation(parts: &[ContentPart]) -> usize {
		if parts.is_empty() {
			return 0;
		}
		let mut reserved = 64_usize.saturating_add(Self::media_tokens(parts));
		// Array/text framing replaces a plain context string; media parts keep
		// their framing but not their encoded payload bytes.
		for part in parts {
			reserved =
				reserved.saturating_add(part.estimate_frame().to_string().len().saturating_add(1));
		}
		reserved
	}

	pub fn media_within_limits(parts: &[ContentPart]) -> bool {
		let mut count = 0_usize;
		let mut bytes = 0_usize;
		for part in parts {
			if let ContentPart::Image { bytes: content, .. }
			| ContentPart::Audio { bytes: content, .. } = part
			{
				count += 1;
				bytes = bytes.saturating_add(content.len());
			}
		}
		count <= 8 && bytes <= 8 * 1024 * 1024
	}

	/// Model-visible payload, shared by transport and context accounting. The
	/// context is encoded as message text, including its JSON escaping. A salted
	/// request's Tenant Cache Salt is prepended by the transport adapter only.
	pub fn input_body(&self) -> Value {
		self.body(
			self.content_parts.iter().map(ContentPart::openrouter),
			self.cache_breakpoints,
		)
	}

	/// `breakpoints` marks `system` and the Ordered Stable Prefix part; it has
	/// no effect on a Legacy context.
	fn body(&self, parts: impl Iterator<Item = Value>, breakpoints: bool) -> Value {
		let mut parts = parts.peekable();
		let text = self.context.text_parts();
		let breakpoints = breakpoints && matches!(self.context, ModelContext::Ordered(_));
		let content = match (&self.context, parts.peek()) {
			(ModelContext::Legacy(_), None) => Value::String(text.concat()),
			_ => Value::Array(
				text.into_iter()
					.enumerate()
					.map(|(index, text)| {
						let mut part = json!({"type":"text","text":text});
						// The Stable Prefix part comes first; the volatile part
						// after it changes every step and is never cached.
						if breakpoints && index == 0 {
							part["cache_control"] = cache_control();
						}
						part
					})
					.chain(parts)
					.collect(),
			),
		};
		let system = if breakpoints {
			json!([{"type":"text","text":self.instructions,"cache_control":cache_control()}])
		} else {
			json!(self.instructions)
		};
		let mut body = json!({"messages":[
			{"role":"system","content":system},
			{"role":"user","content":content}]});
		if !self.tools.is_empty() {
			body["tools"] = Value::Array(self.tools.iter().map(|t| json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.parameters}})).collect());
		}
		body
	}

	/// Conservative UTF-8 byte estimate, not a provider tokenizer. Reserve
	/// completion tokens and framing separately, in the same unit at every gate.
	pub fn estimated_total_tokens(&self) -> usize {
		self.estimated_total_tokens_with_parts(&self.content_parts)
	}

	pub fn estimated_total_tokens_with_parts(&self, parts: &[ContentPart]) -> usize {
		// Base64 is a transport encoding, not text for the model tokenizer.
		// Keep the transmitted array framing, excluding only encoded media
		// payload bytes, and reserve a bounded media estimate.
		// Ordered estimates always count cache breakpoint framing, so whether a
		// step carries breakpoints never changes a fitting decision (ADR 0019).
		let body = self.body(parts.iter().map(ContentPart::estimate_frame), true);
		let salt = if self.cache_scope.is_some() {
			crate::projection::CACHE_SALT_LINE_RESERVE
		} else {
			0
		};
		body.to_string()
			.len()
			.saturating_add(salt)
			.saturating_add(Self::media_tokens(parts))
			.saturating_add(self.max_output_tokens as usize)
			.saturating_add(1024)
	}

	pub fn validate(&self) -> Result<()> {
		if self.cache_breakpoints && !matches!(self.context, ModelContext::Ordered(_)) {
			return Err(Error::Invalid(
				"cache breakpoints require the Ordered Projection Version".into(),
			));
		}
		for part in &self.content_parts {
			part.validate()?;
		}
		if !Self::media_within_limits(&self.content_parts) {
			return Err(Error::Invalid(
				"model media input exceeds count or byte limit".into(),
			));
		}
		Ok(())
	}
}

impl ModelRequest {
	pub fn ensure_fits(&self, window: usize) -> Result<()> {
		Self::check_estimate(
			window,
			self.max_output_tokens,
			self.estimated_total_tokens(),
		)
	}
	pub fn ensure_fits_with_parts(&self, window: usize, parts: &[ContentPart]) -> Result<()> {
		Self::check_estimate(
			window,
			self.max_output_tokens,
			self.estimated_total_tokens_with_parts(parts),
		)
	}
	fn check_estimate(window: usize, max_output_tokens: u32, estimated: usize) -> Result<()> {
		if estimated > window {
			return Err(Error::Invalid(format!(
				"model request exceeds context window: estimated total {estimated}, window {window}, output reserve {}, framing reserve 1024",
				max_output_tokens
			)));
		}
		Ok(())
	}
}

impl ModelRequest {
	/// Bind the exact request metadata and ordered media hashes without persisting media bytes.
	pub fn inference_digest(&self) -> String {
		use sha2::{Digest, Sha256};
		let media = self
			.content_parts
			.iter()
			.map(|part| match part {
				ContentPart::Text(text) => {
					json!({"text":format!("{:x}",Sha256::digest(text.as_bytes()))})
				}
				ContentPart::Image { media_type, bytes } => {
					json!({"media_type":media_type,"digest":format!("{:x}",Sha256::digest(bytes))})
				}
				ContentPart::Audio { format, bytes } => {
					json!({"format":format,"digest":format!("{:x}",Sha256::digest(bytes))})
				}
			})
			.collect::<Vec<_>>();
		crate::registry::rules::digest(&json!({"request":self,"media":media}))
	}
}

#[cfg(test)]
mod admission_tests;
#[cfg(test)]
mod cache_breakpoint_tests;
