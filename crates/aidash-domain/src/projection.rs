//! Projection Versions: the immutable rule that renders a Run's Context
//! Projection into model request bytes (ADR 0015).
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub mod native;

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
#[serde(rename_all = "snake_case")]
/// The rendering used for Run steps and recorded on Usage Records.
pub enum ProjectionVersion {
	/// Version 3: model-native assistant and tool messages, with Model Turns
	/// recorded in the Context Journal (#178).
	#[default]
	Native,
}

impl ProjectionVersion {
	/// The version number recorded on Usage Records (ADR 0015 numbering).
	pub fn number(self) -> u8 {
		3
	}
}

impl std::fmt::Display for ProjectionVersion {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str("native")
	}
}

/// A definition can still carry a retired pin in its immutable JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProjectionPin {
	Active(ProjectionVersion),
	Retired(RetiredProjection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetiredProjection {
	Legacy,
	Ordered,
}

impl std::fmt::Display for RetiredProjection {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(match self {
			Self::Legacy => "legacy",
			Self::Ordered => "ordered",
		})
	}
}

/// An Agent definition's prompt-caching opt-in, pinned with the Run's Binding
/// snapshot (ADR 0019). Omitted means `off`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PromptCache {
	#[default]
	Off,
	/// Send `cache_control` breakpoints at the end of `system` and of the
	/// Stable Prefix part, on a model that declares `explicit` caching.
	Explicit,
}

/// The Cache Scope of one request: the Tenant whose keyed salt starts
/// `system`, and the Cache Salt Key version that derives it. The salt itself
/// is computed only by the transport adapter and never enters the request
/// metadata, compaction state or diagnostics (ADR 0016).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CacheScope {
	pub tenant: String,
	pub key_version: u32,
}

/// Starts every sticky-routing id, followed by 32 lowercase hex digits (ADR 0115).
pub const SESSION_ID_PREFIX: &str = "aidash-session-";

/// Upper bound, in estimate units, for the salt line the adapter prepends:
/// `aidash-cache-scope:v{u32}:{32 hex}\n` with JSON escaping (67 bytes at most).
pub const CACHE_SALT_LINE_RESERVE: usize = 80;

/// The first line of `system` for a salted request. `digest_hex` is the
/// truncated HMAC computed by the adapter.
pub fn cache_salt_line(key_version: u32, digest_hex: &str) -> String {
	format!("aidash-cache-scope:v{key_version}:{digest_hex}\n")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn salt_line_fits_its_estimate_reserve() {
		let line = cache_salt_line(u32::MAX, &"f".repeat(32));
		let encoded = serde_json::to_string(&line).unwrap();
		assert!(
			encoded.len() <= CACHE_SALT_LINE_RESERVE,
			"{}",
			encoded.len()
		);
	}

	#[test]
	fn versions_use_stable_names() {
		assert_eq!(
			serde_json::to_string(&ProjectionVersion::Native).unwrap(),
			"\"native\""
		);
		assert!(serde_json::from_str::<ProjectionVersion>("2").is_err());
	}
}
