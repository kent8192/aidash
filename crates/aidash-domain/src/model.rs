//! Model configuration and accepted media-route invariants.
use crate::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
	pub provider: String,
	pub model_id: String,
	pub endpoint: String,
	pub credential_env: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub provider_credential: Option<String>,
	/// Total inference request timeout in seconds, including the response body.
	/// Omitted or null values use 900 seconds; configured values must be positive.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub request_timeout_secs: Option<u32>,
	#[serde(default)]
	pub reasoning_effort: Option<ReasoningEffort>,
	pub context_window: usize,
	/// Maximum completion tokens reported by the selected provider model.
	/// Missing values are accepted only while reading legacy model versions.
	#[serde(default)]
	pub max_output_tokens: Option<u32>,
	pub modalities: Vec<String>,
	/// Administrator-approved evidence for exact OpenRouter route tags. The
	/// catalog and ZDR APIs are checked again before every media inference.
	#[serde(default)]
	pub media_routes: Vec<MediaRouteEvidence>,
	pub cost: Value,
}

/// OpenRouter's normalized reasoning levels; omission retains the model default.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
	None,
	Minimal,
	Low,
	Medium,
	High,
	Xhigh,
	Max,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaRouteEvidence {
	pub tag: String,
	/// Image MIME types or OpenRouter input_audio format names.
	pub formats: Vec<String>,
	pub source: String,
	pub verified_at: chrono::DateTime<chrono::Utc>,
	pub expires_at: chrono::DateTime<chrono::Utc>,
}

impl ModelConfig {
	pub fn require_media_types<'a>(
		&self,
		media_types: impl IntoIterator<Item = &'a str>,
	) -> Result<()> {
		let mut formats = Vec::new();
		for media_type in media_types {
			if !media_type.starts_with("image/") && !media_type.starts_with("audio/") {
				continue;
			}
			let modality = crate::provider::ContentPart::modality_for_media_type(media_type)?;
			if !self
				.modalities
				.iter()
				.any(|available| available == modality)
			{
				return Err(Error::Invalid(format!(
					"recipient model {} does not support {modality} input",
					self.model_id
				)));
			}
			formats.push(crate::provider::ContentPart::format_for_media_type(
				media_type,
			)?);
		}
		if formats.is_empty() {
			return Ok(());
		}
		if !self.has_current_media_route(formats) {
			return Err(Error::Invalid(format!(
				"recipient model {} has no current media route for every selected format",
				self.model_id
			)));
		}
		Ok(())
	}

	pub fn has_current_media_route_for_parts(
		&self,
		parts: &[crate::provider::ContentPart],
	) -> bool {
		self.has_current_media_route(parts.iter().filter_map(|part| match part {
			crate::provider::ContentPart::Image { media_type, .. } => Some(media_type.as_str()),
			crate::provider::ContentPart::Audio { format, .. } => Some(format.as_str()),
			crate::provider::ContentPart::Text(_) => None,
		}))
	}

	fn has_current_media_route<'a>(&self, formats: impl IntoIterator<Item = &'a str>) -> bool {
		let formats: Vec<_> = formats.into_iter().collect();
		if formats.is_empty() {
			return true;
		}
		self.current_media_routes().any(|route| {
			formats
				.iter()
				.all(|format| route.formats.iter().any(|supported| supported == format))
		})
	}

	fn current_media_routes(&self) -> impl Iterator<Item = &MediaRouteEvidence> {
		let now = chrono::Utc::now();
		self.media_routes
			.iter()
			.filter(move |route| route.verified_at <= now && route.expires_at > now)
	}

	/// Current routes expressed as MIME types the run-message upload accepts.
	pub fn current_media_input_routes(&self) -> Vec<Vec<String>> {
		const MEDIA_TYPES: [&str; 14] = [
			"image/png",
			"image/jpeg",
			"image/gif",
			"image/webp",
			"audio/wav",
			"audio/x-wav",
			"audio/mpeg",
			"audio/mp4",
			"audio/x-m4a",
			"audio/aac",
			"audio/ogg",
			"audio/webm",
			"audio/flac",
			"audio/x-flac",
		];
		self.current_media_routes()
			.map(|route| {
				MEDIA_TYPES
					.into_iter()
					.filter(|media_type| {
						let modality =
							crate::provider::ContentPart::modality_for_media_type(media_type)
								.expect("supported media type");
						let format =
							crate::provider::ContentPart::format_for_media_type(media_type)
								.expect("supported media type");
						self.modalities
							.iter()
							.any(|available| available == modality)
							&& route.formats.iter().any(|supported| supported == format)
					})
					.map(str::to_owned)
					.collect::<Vec<_>>()
			})
			.filter(|route| !route.is_empty())
			.collect()
	}
	/// Resolve the provider's inference deadline without inheriting the shared
	/// HTTP client's shorter default. Validate at registration and before use.
	pub fn request_timeout(&self) -> Result<Duration> {
		let seconds = self.request_timeout_secs.unwrap_or(900);
		if seconds == 0 {
			return Err(Error::Invalid(
				"model request_timeout_secs must be greater than zero".into(),
			));
		}
		Ok(Duration::from_secs(u64::from(seconds)))
	}

	/// Preserve the historical output allowance for model versions registered
	/// before their provider limit was captured in the immutable config.
	pub fn output_token_limit(&self) -> u32 {
		self.max_output_tokens
			.unwrap_or_else(|| (self.context_window / 8).clamp(256, 4096) as u32)
	}
}
