//! Sealed Tenant Secret payloads (ADR 0096). On Cloud and Dedicated a Secret
//! Store version holds the value together with the hosts it may be sent to, so
//! a compromised host cannot point a stored value anywhere new.
//!
//! The payload is UTF-8 JSON `{"v":1,"hosts":[...],"value":"..."}`. Hosts are
//! bare, lowercase and normalized exactly as `aidash_domain::tenant_secrets`
//! does; `aidash-capability` stays a leaf crate, so the rule is repeated here
//! and a test in a crate that sees both keeps the two in step.
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// The payload version this code writes and reads.
pub const VERSION: u32 = 1;
/// The most hosts one Tenant Secret may record.
pub const MAX_HOSTS: usize = 16;
/// A sealed payload larger than this is never a Tenant Secret this code wrote.
const MAX_SEALED_BYTES: usize = 65_536;

#[derive(Serialize)]
struct SealPayload<'a> {
	v: u32,
	hosts: &'a [String],
	value: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenPayload {
	v: u32,
	hosts: Vec<String>,
	value: SecretString,
}

/// What a sealed payload holds. It never derives `Debug`, so a value cannot be
/// printed by accident.
pub struct Sealed {
	pub hosts: Vec<String>,
	pub value: SecretString,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid sealed Tenant Secret payload")]
pub struct SealedError;

/// Seals a value with the hosts it may be sent to. Hosts are normalized,
/// de-duplicated and sorted here, and the output is zeroized when dropped.
///
/// # Panics
///
/// `hosts` must be valid Tenant Secret hosts (1 to `MAX_HOSTS`); sealing an
/// invalid list is a programming error, not a call the caller may retry.
pub fn seal(hosts: &[String], value: &str) -> Zeroizing<Vec<u8>> {
	let hosts = normalize_hosts(hosts).expect("Tenant Secret hosts are validated before sealing");
	let payload = SealPayload {
		v: VERSION,
		hosts: &hosts,
		value,
	};
	Zeroizing::new(serde_json::to_vec(&payload).expect("a sealed Tenant Secret always serializes"))
}

/// Opens a payload the Secret Store returned. A payload that is not exactly a
/// sealed Tenant Secret this code wrote is refused whole.
pub fn open(bytes: &[u8]) -> Result<Sealed, SealedError> {
	if bytes.is_empty() || bytes.len() > MAX_SEALED_BYTES {
		return Err(SealedError);
	}
	let payload: OpenPayload = serde_json::from_slice(bytes).map_err(|_| SealedError)?;
	if payload.v != VERSION {
		return Err(SealedError);
	}
	let hosts = normalize_hosts(&payload.hosts).ok_or(SealedError)?;
	if hosts != payload.hosts {
		return Err(SealedError);
	}
	Ok(Sealed {
		hosts,
		value: payload.value,
	})
}

/// A bare, lowercase DNS name or IP literal, without scheme, port, path or
/// credentials. Kept identical to `aidash_domain::tenant_secrets::normalize_host`.
fn normalize_host(host: &str) -> Option<String> {
	let host = host.trim().to_ascii_lowercase();
	if host.is_empty() || host.len() > 253 || host.contains(['/', '@', ':', '?', '#', ' ', '*']) {
		return None;
	}
	// Reuse the URL host grammar so that recorded and requested hosts agree.
	let parsed = url::Url::parse(&format!("https://{host}/")).ok()?;
	(parsed.host_str() == Some(host.as_str())).then_some(host)
}

/// Kept identical to `aidash_domain::tenant_secrets::normalize_hosts`: 1 to
/// `MAX_HOSTS` inputs, normalized, de-duplicated and sorted.
fn normalize_hosts(hosts: &[String]) -> Option<Vec<String>> {
	if hosts.is_empty() || hosts.len() > MAX_HOSTS {
		return None;
	}
	let mut normalized = Vec::with_capacity(hosts.len());
	for host in hosts {
		let host = normalize_host(host)?;
		if !normalized.contains(&host) {
			normalized.push(host);
		}
	}
	normalized.sort();
	Some(normalized)
}

#[cfg(test)]
mod tests;
