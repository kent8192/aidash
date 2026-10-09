//! Admission rules for provider-side input processing outside signed authority.
use aidash_capability::Failure;
use aidash_domain::provider::{ContentPart, ModelRequest};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;

fn has_cache_control(value: &Value) -> bool {
	match value {
		Value::Object(fields) => {
			fields.contains_key("cache_control") || fields.values().any(has_cache_control)
		}
		Value::Array(values) => values.iter().any(has_cache_control),
		_ => false,
	}
}

fn media(part: &Value) -> Result<ContentPart, Failure> {
	let (mime, encoded) = match part.get("type").and_then(Value::as_str) {
		Some("image_url") => {
			let (mime, encoded) = part
				.pointer("/image_url/url")
				.and_then(Value::as_str)
				.and_then(|url| url.strip_prefix("data:"))
				.and_then(|url| url.split_once(";base64,"))
				.ok_or(Failure::ClaimViolation)?;
			if ContentPart::modality_for_media_type(mime).ok() != Some("image") {
				return Err(Failure::ClaimViolation);
			}
			(mime, encoded)
		}
		Some("input_audio") => {
			let mime = match part.pointer("/input_audio/format").and_then(Value::as_str) {
				Some("wav") => "audio/wav",
				Some("mp3") => "audio/mpeg",
				Some("m4a") => "audio/mp4",
				Some("aac") => "audio/aac",
				Some("ogg") => "audio/ogg",
				Some("webm") => "audio/webm",
				Some("flac") => "audio/flac",
				_ => return Err(Failure::ClaimViolation),
			};
			let encoded = part
				.pointer("/input_audio/data")
				.and_then(Value::as_str)
				.ok_or(Failure::ClaimViolation)?;
			(mime, encoded)
		}
		_ => return Err(Failure::ClaimViolation),
	};
	let bytes = STANDARD
		.decode(encoded)
		.map_err(|_| Failure::ClaimViolation)?;
	ContentPart::from_media(mime, bytes).map_err(|_| Failure::ClaimViolation)
}

pub(crate) fn chat_input(value: &Value) -> Result<(), Failure> {
	// JSON parsing bounds nesting; inspect keys, not ordinary message text.
	if has_cache_control(value) {
		return Err(Failure::ClaimViolation);
	}
	let Some(messages) = value.get("messages") else {
		return Ok(());
	};
	let messages = messages.as_array().ok_or(Failure::ClaimViolation)?;
	let mut parts = Vec::new();
	for message in messages {
		match message.get("content") {
			None | Some(Value::Null | Value::String(_)) => {}
			Some(Value::Array(content)) => {
				for part in content {
					if part.get("type").and_then(Value::as_str) == Some("text") {
						if part.get("text").and_then(Value::as_str).is_none() {
							return Err(Failure::ClaimViolation);
						}
						continue;
					}
					parts.push(media(part)?);
					// Count and decoded bytes are bounded across every message.
					if !ModelRequest::media_within_limits(&parts) {
						return Err(Failure::ClaimViolation);
					}
				}
			}
			_ => return Err(Failure::ClaimViolation),
		}
	}
	Ok(())
}
