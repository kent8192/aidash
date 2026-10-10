//! Tenant Cache Salt (ADR 0016): the keyed first line of `system` that keeps
//! provider prompt caches from being shared across Tenants. The salt exists
//! only inside the outgoing request body; it is never logged or returned.
use crate::{Error, Result};
use aidash_domain::projection::{CacheScope, cache_salt_line};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{collections::BTreeMap, fmt, fmt::Write as _, sync::Arc};

/// One Cache Salt Key as configured in node settings. `Debug` shows only the
/// version, never the secret.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheSaltKey {
	pub version: u32,
	pub secret: String,
}

impl fmt::Debug for CacheSaltKey {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("CacheSaltKey")
			.field("version", &self.version)
			.finish_non_exhaustive()
	}
}

/// This node's validated Cache Salt Keys and the version new requests use.
/// Rotation changes only `current`; retained versions stay derivable.
#[derive(Clone)]
pub struct CacheSaltKeys {
	keys: Arc<BTreeMap<u32, Hmac<Sha256>>>,
	current: u32,
}

impl CacheSaltKeys {
	/// Rejects empty secrets, duplicate versions and a `current` version that
	/// names no configured key.
	pub fn new(keys: &[CacheSaltKey], current: u32) -> Result<Self> {
		let mut macs = BTreeMap::new();
		for key in keys {
			if key.secret.trim().is_empty() {
				return Err(Error::Invalid(format!(
					"cache_salt key v{} has an empty secret",
					key.version
				)));
			}
			let mac = Hmac::<Sha256>::new_from_slice(key.secret.as_bytes())
				.map_err(|_| Error::Invalid("invalid cache_salt key".into()))?;
			if macs.insert(key.version, mac).is_some() {
				return Err(Error::Invalid(format!(
					"cache_salt key v{} is configured more than once",
					key.version
				)));
			}
		}
		if !macs.contains_key(&current) {
			return Err(Error::Invalid(format!(
				"cache_salt.current v{current} names no configured key"
			)));
		}
		Ok(Self {
			keys: Arc::new(macs),
			current,
		})
	}

	/// The key version new Cache Scopes use.
	pub fn current(&self) -> u32 {
		self.current
	}

	/// The salt line that starts `system` for `scope`: HMAC-SHA256 of the
	/// Tenant id keyed by the scope's key version, truncated to 128 bits.
	pub fn line(&self, scope: &CacheScope) -> Result<String> {
		let mut mac = self
			.keys
			.get(&scope.key_version)
			.ok_or_else(|| {
				Error::Invalid(format!(
					"Cache Salt Key v{} is not configured on this node",
					scope.key_version
				))
			})?
			.clone();
		mac.update(scope.tenant.as_bytes());
		let digest = mac.finalize().into_bytes();
		let mut hex = String::with_capacity(32);
		for byte in &digest[..16] {
			let _ = write!(hex, "{byte:02x}");
		}
		Ok(cache_salt_line(scope.key_version, &hex))
	}
}

impl fmt::Debug for CacheSaltKeys {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("CacheSaltKeys")
			.field("versions", &self.keys.keys().collect::<Vec<_>>())
			.field("current", &self.current)
			.finish()
	}
}
