//! Admission rules for provider-side input processing outside signed authority.
use std::collections::HashSet;

use aidash_capability::Failure;
use aidash_domain::provider::{ContentPart, ModelRequest};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Value, json};

/// Provider limit on explicit prompt-cache breakpoints per request.
const MAX_CACHE_BREAKPOINTS: usize = 4;

/// Parses an inbound body for admission, rejecting duplicate object members.
///
/// `Value` keeps only the last duplicate, but the broker relays the original bytes;
/// an upstream parser that keeps the first member would see unauthorized fields.
pub(crate) fn parse(body: &[u8]) -> Result<Value, Failure> {
	serde_json::from_slice::<UniqueMembers>(body).map_err(|_| Failure::ClaimViolation)?;
	serde_json::from_slice(body).map_err(|_| Failure::ClaimViolation)
}

/// Validates that no JSON object at any depth repeats a member name.
struct UniqueMembers;

impl<'de> Deserialize<'de> for UniqueMembers {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		deserializer.deserialize_any(UniqueMembersVisitor)
	}
}

struct UniqueMembersVisitor;

impl<'de> Visitor<'de> for UniqueMembersVisitor {
	type Value = UniqueMembers;

	fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str("JSON without duplicate object members")
	}

	fn visit_bool<E>(self, _: bool) -> Result<UniqueMembers, E> {
		Ok(UniqueMembers)
	}

	fn visit_i64<E>(self, _: i64) -> Result<UniqueMembers, E> {
		Ok(UniqueMembers)
	}

	fn visit_u64<E>(self, _: u64) -> Result<UniqueMembers, E> {
		Ok(UniqueMembers)
	}

	fn visit_f64<E>(self, _: f64) -> Result<UniqueMembers, E> {
		Ok(UniqueMembers)
	}

	fn visit_str<E>(self, _: &str) -> Result<UniqueMembers, E> {
		Ok(UniqueMembers)
	}

	fn visit_unit<E>(self) -> Result<UniqueMembers, E> {
		Ok(UniqueMembers)
	}

	fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<UniqueMembers, A::Error> {
		while seq.next_element::<UniqueMembers>()?.is_some() {}
		Ok(UniqueMembers)
	}

	fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<UniqueMembers, A::Error> {
		let mut keys = HashSet::new();
		while let Some(key) = map.next_key::<String>()? {
			if !keys.insert(key) {
				return Err(serde::de::Error::custom("duplicate object member"));
			}
			map.next_value::<UniqueMembers>()?;
		}
		Ok(UniqueMembers)
	}
}

/// Counts every `cache_control` key at any depth, whatever its value.
fn count_cache_control(value: &Value) -> usize {
	match value {
		Value::Object(fields) => fields
			.iter()
			.map(|(key, field)| usize::from(key == "cache_control") + count_cache_control(field))
			.sum(),
		Value::Array(values) => values.iter().map(count_cache_control).sum(),
		_ => 0,
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

/// `cache_breakpoints` is the signed authority for explicit prompt-cache writes. Only
/// exactly `{"type":"ephemeral"}` on a `messages[*].content[*]` text part is allowed,
/// at most four times; every other `cache_control` key is rejected.
pub(crate) fn chat_input(value: &Value, cache_breakpoints: bool) -> Result<(), Failure> {
	// Each admitted breakpoint is counted below; any `cache_control` key left over
	// sits somewhere else (tools, message level, non-text part, nested) and is rejected.
	// JSON parsing bounds nesting; inspect keys, not ordinary message text.
	let mut breakpoints = 0;
	chat_messages(value, cache_breakpoints, &mut breakpoints)?;
	if count_cache_control(value) != breakpoints {
		return Err(Failure::ClaimViolation);
	}
	Ok(())
}

fn chat_messages(
	value: &Value,
	cache_breakpoints: bool,
	breakpoints: &mut usize,
) -> Result<(), Failure> {
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
						if let Some(control) = part.get("cache_control") {
							*breakpoints += 1;
							if !cache_breakpoints
								|| *breakpoints > MAX_CACHE_BREAKPOINTS
								|| control != &json!({"type": "ephemeral"})
							{
								return Err(Failure::ClaimViolation);
							}
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

/// A Responses `input_image` carries one data URL. Remote URLs, file IDs and
/// every other input part are outside the signed inference authority.
fn response_media(part: &Value) -> Result<ContentPart, Failure> {
	let (mime, encoded) = part
		.get("image_url")
		.and_then(Value::as_str)
		.and_then(|url| url.strip_prefix("data:"))
		.and_then(|url| url.split_once(";base64,"))
		.ok_or(Failure::ClaimViolation)?;
	if ContentPart::modality_for_media_type(mime).ok() != Some("image") {
		return Err(Failure::ClaimViolation);
	}
	let bytes = STANDARD
		.decode(encoded)
		.map_err(|_| Failure::ClaimViolation)?;
	ContentPart::from_media(mime, bytes).map_err(|_| Failure::ClaimViolation)
}

/// A Responses body may carry only model-visible messages and the client's own
/// function calls. It has no `messages` field, and a breakpoint is allowed only
/// on an `input_text` or `output_text` part, at most four times, exactly as in
/// chat. Any other `cache_control` key is rejected below.
pub(crate) fn responses_input(value: &Value, cache_breakpoints: bool) -> Result<(), Failure> {
	if value.get("messages").is_some() {
		return Err(Failure::ClaimViolation);
	}
	let mut breakpoints = 0;
	let mut parts = Vec::new();
	match value.get("input") {
		None | Some(Value::Null) => {}
		Some(Value::String(_)) => {}
		Some(Value::Array(items)) => {
			for item in items {
				match item.get("type").and_then(Value::as_str) {
					Some("function_call") => {
						for key in ["call_id", "name", "arguments"] {
							if item.get(key).and_then(Value::as_str).is_none() {
								return Err(Failure::ClaimViolation);
							}
						}
					}
					Some("function_call_output") => {
						if item.get("call_id").and_then(Value::as_str).is_none()
							|| item.get("output").and_then(Value::as_str).is_none()
						{
							return Err(Failure::ClaimViolation);
						}
					}
					Some(kind) if aidash_domain::tool::provider_tools::replayable(kind) => {}
					Some("message") | None if item.get("role").is_some() => {
						response_message(item, cache_breakpoints, &mut breakpoints, &mut parts)?
					}
					_ => return Err(Failure::ClaimViolation),
				}
			}
		}
		_ => return Err(Failure::ClaimViolation),
	}
	if count_cache_control(value) != breakpoints {
		return Err(Failure::ClaimViolation);
	}
	Ok(())
}

fn response_message(
	item: &Value,
	cache_breakpoints: bool,
	breakpoints: &mut usize,
	parts: &mut Vec<ContentPart>,
) -> Result<(), Failure> {
	match item.get("content") {
		None | Some(Value::Null | Value::String(_)) => Ok(()),
		Some(Value::Array(content)) => {
			for part in content {
				match part.get("type").and_then(Value::as_str) {
					Some("input_text" | "output_text") => {
						if part.get("text").and_then(Value::as_str).is_none() {
							return Err(Failure::ClaimViolation);
						}
						if let Some(control) = part.get("cache_control") {
							*breakpoints += 1;
							if !cache_breakpoints
								|| *breakpoints > MAX_CACHE_BREAKPOINTS
								|| control != &json!({"type": "ephemeral"})
							{
								return Err(Failure::ClaimViolation);
							}
						}
					}
					Some("input_image") => {
						parts.push(response_media(part)?);
						if !ModelRequest::media_within_limits(parts) {
							return Err(Failure::ClaimViolation);
						}
					}
					_ => return Err(Failure::ClaimViolation),
				}
			}
			Ok(())
		}
		_ => Err(Failure::ClaimViolation),
	}
}

/// A decision body may carry only the decision itself and its routing. `trace`,
/// `user` and `session_id` reach OpenRouter's observability destinations, and
/// any other `provider` preference could widen routing. A System One body routes
/// only by zero data retention, with a token that names no hosts; a Decisions
/// API body names exactly the hosts its token signs from the Decider Route, and
/// OpenRouter may move only between those (ADR 0066).
pub(crate) fn decision_input(
	value: &Value,
	decisions: bool,
	hosts: &[String],
	requirement: aidash_domain::data_protection::Requirement,
) -> Result<(), Failure> {
	let fields = value.as_object().ok_or(Failure::ClaimViolation)?;
	let provider = fields.get("provider");
	let mut expected = requirement.provider();
	if decisions {
		expected["only"] = json!(hosts);
		expected["allow_fallbacks"] = json!(true);
	}
	let routed = (if decisions {
		!hosts.is_empty()
	} else {
		hosts.is_empty()
	}) && provider == Some(&expected);
	if fields
		.keys()
		.any(|key| !matches!(key.as_str(), "model" | "state" | "questions" | "provider"))
		|| !fields.contains_key("state")
		|| fields
			.get("questions")
			.and_then(Value::as_object)
			.is_none_or(|questions| questions.is_empty())
		|| !routed
	{
		return Err(Failure::ClaimViolation);
	}
	Ok(())
}
