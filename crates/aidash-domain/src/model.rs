//! Model configuration and accepted media-route invariants.
use crate::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// Streaming applies to model configs that do not choose explicitly.
pub const STREAMING_DEFAULT: bool = true;

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
	/// BYOK configurations are capped at the broker's 3600-second deadline.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub request_timeout_secs: Option<u32>,
	/// Stream the provider response and publish Inference Progress. Omitted
	/// values use the release default; `false` keeps the non-streamed path.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub streaming: Option<bool>,
	/// Longest silence, in seconds, between streamed data chunks. Provider
	/// comment lines do not count as data. Omitted values use 120 seconds.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub stream_stall_timeout_secs: Option<u32>,
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
	/// Projection Versions this model accepts (ADR 0015). Omitted means Legacy
	/// only and is not serialized, so existing model definitions keep bytes.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub projection_versions: Vec<crate::projection::ProjectionVersion>,
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
	/// Projection Versions this model accepts; Legacy when none are declared.
	pub fn supports_projection(&self, version: crate::projection::ProjectionVersion) -> bool {
		if self.projection_versions.is_empty() {
			version.is_legacy()
		} else {
			self.projection_versions.contains(&version)
		}
	}

	/// Model definitions may declare only implemented versions, each once.
	pub fn validate_projection_versions(&self) -> Result<()> {
		let mut declared = std::collections::BTreeSet::new();
		for version in &self.projection_versions {
			if !version.is_implemented() {
				return Err(Error::Invalid(format!(
					"model {} declares Projection Version {version}, which is not implemented",
					self.model_id
				)));
			}
			if !declared.insert(version) {
				return Err(Error::Invalid(format!(
					"model {} declares Projection Version {version} more than once",
					self.model_id
				)));
			}
		}
		Ok(())
	}

	/// An Agent may pin only an implemented version its model declares.
	pub fn require_projection(&self, version: crate::projection::ProjectionVersion) -> Result<()> {
		if !version.is_implemented() {
			return Err(Error::Invalid(format!(
				"Projection Version {version} is not implemented"
			)));
		}
		if !self.supports_projection(version) {
			return Err(Error::Invalid(format!(
				"model {} does not declare Projection Version {version}",
				self.model_id
			)));
		}
		Ok(())
	}

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
		if self.provider_credential.is_some() && seconds > 3600 {
			return Err(Error::Invalid(
				"BYOK model request_timeout_secs must be at most 3600".into(),
			));
		}
		Ok(Duration::from_secs(u64::from(seconds)))
	}

	/// Whether inference streams progress. The release default applies when the
	/// immutable config omits the choice.
	pub fn streaming(&self) -> bool {
		self.streaming.unwrap_or(STREAMING_DEFAULT)
	}

	/// Resolve the streamed-response stall timeout. Validate at registration and
	/// before use.
	pub fn stream_stall_timeout(&self) -> Result<Duration> {
		let seconds = self.stream_stall_timeout_secs.unwrap_or(120);
		if seconds == 0 {
			return Err(Error::Invalid(
				"model stream_stall_timeout_secs must be greater than zero".into(),
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
