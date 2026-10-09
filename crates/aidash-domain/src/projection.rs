//! Projection Versions: the immutable rule that renders a Run's Context
//! Projection into model request bytes (ADR 0015).
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Named by an Agent definition and fixed for a Run through its Binding
/// snapshot. Each value is a complete, frozen rendering.
#[derive(
	Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionVersion {
	/// Version 1: one alphabetical JSON user message. Definitions and
	/// snapshots without the field use it, byte for byte.
	#[default]
	Legacy,
	/// Version 2: one user message with a Stable Prefix part and a volatile
	/// part, and a Tenant Cache Salt at the start of `system` (#172).
	Ordered,
	/// Version 3: model-native assistant and tool messages. Reserved for #178.
	Native,
}

impl ProjectionVersion {
	pub fn is_legacy(&self) -> bool {
		*self == Self::Legacy
	}

	/// Whether this build can render the version. `Native` is reserved until
	/// its renderer exists, so registration and Run creation reject it.
	pub fn is_implemented(&self) -> bool {
		matches!(self, Self::Legacy | Self::Ordered)
	}

	pub fn salted(&self) -> bool {
		!self.is_legacy()
	}
}

impl std::fmt::Display for ProjectionVersion {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(match self {
			Self::Legacy => "legacy",
			Self::Ordered => "ordered",
			Self::Native => "native",
		})
	}
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

/// Upper bound, in estimate units, for the salt line the adapter prepends:
/// `aidash-cache-scope:v{u32}:{32 hex}\n` plus JSON escaping.
pub const CACHE_SALT_LINE_RESERVE: usize = 64;

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
		assert!(encoded.len() <= CACHE_SALT_LINE_RESERVE, "{}", encoded.len());
	}

	#[test]
	fn versions_use_stable_names() {
		assert_eq!(
			serde_json::to_string(&ProjectionVersion::Ordered).unwrap(),
			"\"ordered\""
		);
		assert!(serde_json::from_str::<ProjectionVersion>("2").is_err());
	}
}
