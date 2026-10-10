//! The node-local prompt cache key (ADR 0016). It separates provider prompt
//! caches per Tenant through the salt line that opens an `Ordered` system
//! prompt. It is node configuration, not Key Material, and never leaves the
//! process: only its version may appear in diagnostics.
use crate::{Error, Result};
use aidash_domain::context::projection::{CACHE_SALT_MAC_BYTES, cache_salt_line};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::{fmt, sync::Arc};

/// Domain separation for every prompt cache scope MAC.
const SCOPE_DOMAIN: &[u8] = b"aidash.prompt-cache-scope.v1\0";

/// The authority a Run's provider requests are cached under.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheScope<'a> {
	/// The Tenant whose authority executes the Run. Tenant names are local to
	/// the executing Node, so the Node is part of the scope: equal Tenant names
	/// on Nodes sharing a key and provider account remain separate scopes.
	Tenant { node: &'a str, tenant: &'a str },
	/// A Run without Tenant authority, scoped to the executing Node.
	Operator(&'a str),
}

impl PromptCacheScope<'_> {
	fn update(self, mac: &mut Hmac<Sha256>) {
		mac.update(SCOPE_DOMAIN);
		match self {
			Self::Tenant { node, tenant } => {
				mac.update(b"tenant\0");
				update_field(mac, node);
				update_field(mac, tenant);
			}
			Self::Operator(node) => {
				mac.update(b"operator\0");
				update_field(mac, node);
			}
		}
	}
}

/// Length-prefix each identity so no two field splits share one MAC input.
fn update_field(mac: &mut Hmac<Sha256>, value: &str) {
	mac.update(&(value.len() as u64).to_be_bytes());
	mac.update(value.as_bytes());
}

/// A versioned HMAC-SHA256 key. `Debug` shows only the version.
#[derive(Clone)]
pub struct PromptCacheKey {
	version: u32,
	key: Arc<[u8]>,
}

impl PromptCacheKey {
	/// Accept the key from node configuration. It follows the peer credential
	/// rule: at least 32 printable ASCII characters and 8 distinct characters,
	/// used verbatim as the HMAC key. Version 0 is reserved for request
	/// estimates made without a Tenant.
	pub fn new(version: u32, key: &str) -> Result<Self> {
		validate_prompt_cache_key(key)?;
		if version == 0 {
			return Err(Error::Invalid(
				"node.prompt_cache_key_version must be at least 1".into(),
			));
		}
		Ok(Self {
			version,
			key: Arc::from(key.as_bytes()),
		})
	}

	pub fn version(&self) -> u32 {
		self.version
	}

	/// The first `system` line of an `Ordered` request for this scope.
	pub fn salt_line(&self, scope: PromptCacheScope<'_>) -> Result<String> {
		let mut mac = Hmac::<Sha256>::new_from_slice(&self.key)
			.map_err(|_| Error::External("prompt cache salt unavailable".into()))?;
		scope.update(&mut mac);
		let mut digest = [0; CACHE_SALT_MAC_BYTES];
		digest.copy_from_slice(&mac.finalize().into_bytes());
		Ok(cache_salt_line(self.version, &digest))
	}
}

impl fmt::Debug for PromptCacheKey {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("PromptCacheKey")
			.field("version", &self.version)
			.finish_non_exhaustive()
	}
}

/// The salt line for `scope`. Without a node key this fails: an `Ordered`
/// request never goes out unsalted or with a constant salt.
pub fn salt(key: Option<&PromptCacheKey>, scope: PromptCacheScope<'_>) -> Result<String> {
	key.ok_or_else(|| {
		Error::Invalid("Ordered projection requires a configured prompt cache key".into())
	})?
	.salt_line(scope)
}

/// The key must be strong; its value never appears in the error.
pub fn validate_prompt_cache_key(value: &str) -> Result<()> {
	if value.len() < 32
		|| !value.bytes().all(|b| b.is_ascii_graphic())
		|| value
			.bytes()
			.collect::<std::collections::HashSet<_>>()
			.len() < 8
	{
		return Err(Error::Invalid(
			"node.prompt_cache_key requires at least 32 printable ASCII characters and 8 distinct characters; use a randomly generated key".into(),
		));
	}
	Ok(())
}

#[cfg(test)]
mod tests;
