//! Model configuration and accepted media-route invariants.
use crate::{Error, Result, context::projection::ProjectionVersion};
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
	/// BYOK configurations are capped at the broker's 3600-second deadline.
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
	/// Projection Versions an Agent on this model may pin (ADR 0015).
	#[serde(
		default = "ProjectionVersion::legacy_only",
		skip_serializing_if = "ProjectionVersion::is_legacy_only"
	)]
	pub projection_versions: Vec<ProjectionVersion>,
	/// How the provider route caches prompt prefixes (ADR 0019). Only
	/// `explicit` lets an opted-in Agent send cache breakpoints.
	#[serde(default, skip_serializing_if = "CacheMode::is_none")]
	pub cache_mode: CacheMode,
}

/// A model route's declared prompt-caching behavior.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CacheMode {
	/// No prompt caching is assumed.
	#[default]
	None,
	/// The provider caches matching prefixes without request markers.
	Automatic,
	/// The provider caches only up to request breakpoints (`cache_control`).
	Explicit,
}

impl CacheMode {
	pub fn is_none(&self) -> bool {
		*self == Self::None
	}
}

/// OpenRouter model slug prefixes whose routes accept `cache_control`
/// breakpoints. A declaration alone never sends them elsewhere (ADR 0019).
pub const EXPLICIT_CACHE_PREFIXES: &[&str] = &["anthropic/"];

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
	pub fn supports_projection(&self, version: ProjectionVersion) -> bool {
		self.projection_versions.contains(&version)
	}

	/// Whether this route may receive `cache_control` breakpoints.
	pub fn accepts_cache_breakpoints(&self) -> bool {
		self.cache_mode == CacheMode::Explicit
			&& EXPLICIT_CACHE_PREFIXES
				.iter()
				.any(|prefix| self.model_id.starts_with(prefix))
	}

	/// An `explicit` declaration is accepted only for allowlisted slugs.
	pub fn validate_cache_mode(&self) -> Result<()> {
		if self.cache_mode == CacheMode::Explicit && !self.accepts_cache_breakpoints() {
			return Err(Error::Invalid(format!(
				"model {} cannot declare explicit prompt caching; supported slug prefixes: {}",
				self.model_id,
				EXPLICIT_CACHE_PREFIXES.join(", ")
			)));
		}
		Ok(())
	}

	/// Supported Projection Versions are a non-empty set.
	pub fn validate_projection_versions(&self) -> Result<()> {
		let unique: std::collections::BTreeSet<_> = self.projection_versions.iter().collect();
		if unique.is_empty() || unique.len() != self.projection_versions.len() {
			return Err(Error::Invalid(
				"model projection_versions must be a non-empty list without duplicates".into(),
			));
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

	/// Preserve the historical output allowance for model versions registered
	/// before their provider limit was captured in the immutable config.
	pub fn output_token_limit(&self) -> u32 {
		self.max_output_tokens
			.unwrap_or_else(|| (self.context_window / 8).clamp(256, 4096) as u32)
	}
}
