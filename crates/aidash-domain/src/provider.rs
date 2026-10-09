//! Inference inputs, outputs, media validation, and request budgets.
use crate::{Error, Result};
use base64::Engine;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::context::projection::ProjectionVersion;

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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModelRequest {
	pub instructions: String,
	pub context: Value,
	pub tools: Vec<ToolSpec>,
	pub max_output_tokens: u32,
	/// How `context` becomes user-message text. Omitted for `Legacy`, so legacy
	/// request metadata and digests keep their exact bytes.
	#[serde(default, skip_serializing_if = "ProjectionVersion::is_legacy")]
	pub projection: ProjectionVersion,
	/// Resolved, authorized input for this inference only. Never persist bytes
	/// in the durable context or serialize them with the request metadata.
	#[serde(skip)]
	pub content_parts: Vec<ContentPart>,
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
	/// labels and the conservative provider-side media token allowance.
	pub fn content_parts_reservation(parts: &[ContentPart]) -> usize {
		if parts.is_empty() {
			return 0;
		}
		let mut reserved = 64_usize.saturating_add(Self::media_tokens(parts));
		// Array/text framing replaces a plain context string.
		for part in parts {
			if let ContentPart::Text(_) = part {
				reserved =
					reserved.saturating_add(part.openrouter().to_string().len().saturating_add(1));
			}
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

	/// User-message content: the rendered context followed by `extra` parts.
	/// `Legacy` sends one context string, or one text part when media follows;
	/// `Ordered` always sends its text parts (ADR 0015).
	fn user_content(&self, extra: Vec<Value>) -> Value {
		match self.projection {
			ProjectionVersion::Legacy if extra.is_empty() => {
				Value::String(self.context.to_string())
			}
			ProjectionVersion::Legacy => Value::Array(
				std::iter::once(json!({"type":"text","text":self.context.to_string()}))
					.chain(extra)
					.collect(),
			),
			ProjectionVersion::Ordered => Value::Array(
				crate::context::projection::ordered_texts(&self.context)
					.into_iter()
					.map(|text| json!({"type":"text","text":text}))
					.chain(extra)
					.collect(),
			),
		}
	}

	fn body(&self, content: Value) -> Value {
		let mut body = json!({"messages":[
			{"role":"system","content":self.instructions},
			{"role":"user","content":content}]});
		if !self.tools.is_empty() {
			body["tools"] = Value::Array(self.tools.iter().map(|t| json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.parameters}})).collect());
		}
		body
	}

	/// Model-visible payload, shared by transport and context accounting. The
	/// context is encoded as message text, including its JSON escaping.
	pub fn input_body(&self) -> Value {
		self.body(
			self.user_content(
				self.content_parts
					.iter()
					.map(ContentPart::openrouter)
					.collect(),
			),
		)
	}

	/// Conservative UTF-8 byte estimate, not a provider tokenizer. Reserve
	/// completion tokens and framing separately, in the same unit at every gate.
	pub fn estimated_total_tokens(&self) -> usize {
		self.estimated_total_tokens_with_parts(&self.content_parts)
	}

	pub fn estimated_total_tokens_with_parts(&self, parts: &[ContentPart]) -> usize {
		// Base64 is a transport encoding, not text for the model tokenizer.
		// Keep the ordinary text estimate and reserve a bounded media estimate.
		let text_parts = parts
			.iter()
			.filter_map(|part| match part {
				ContentPart::Text(_) => Some(part.openrouter()),
				_ => None,
			})
			.collect();
		// Legacy switches to array framing for any part, including media.
		let content = if parts.is_empty() || !self.projection.is_legacy() {
			self.user_content(text_parts)
		} else {
			match self.user_content(text_parts) {
				Value::String(text) => Value::Array(vec![json!({"type":"text","text":text})]),
				content => content,
			}
		};
		self.body(content)
			.to_string()
			.len()
			.saturating_add(Self::media_tokens(parts))
			.saturating_add(self.max_output_tokens as usize)
			.saturating_add(1024)
	}

	pub fn validate(&self) -> Result<()> {
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
