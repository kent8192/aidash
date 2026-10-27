//! Tenant Secrets (ADR 0051): named, write-only values a Tenant stores so
//! that its own tool definitions can send them, and only to the hosts recorded
//! with each. The value lives in the Secret Store and is never returned.
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What a tool call fails with when the Tenant Secret it names is missing,
/// deleted or no longer permitted to reach the tool's host.
pub const UNAVAILABLE: &str = "secret_unavailable";
/// The most hosts one Tenant Secret may record.
pub const MAX_HOSTS: usize = 16;
/// The warning Catalog Approval states beside every Tenant Secret a definition
/// would send (#296): the call goes outside Aidash's own providers.
pub const APPROVAL_WARNING: &str = "Aidash's guarantees, such as zero data retention and Usage Records, do not apply to calls that carry a Tenant Secret.";

/// A Tenant Secret's name: what definitions write in `secret`.
pub fn validate_name(name: &str) -> Result<()> {
	let mut chars = name.chars();
	let first = chars.next();
	if name.len() > 64
		|| !first.is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
		|| !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
	{
		return Err(Error::Invalid(
			"a Tenant Secret name has 1 to 64 lowercase letters, digits, '-' or '_' and starts with a letter or digit".into(),
		));
	}
	Ok(())
}

/// A recorded host: a lowercase DNS name or IP literal, without scheme, port,
/// path or credentials. Any port of a recorded host is permitted.
pub fn normalize_host(host: &str) -> Result<String> {
	let invalid = || Error::Invalid("a Tenant Secret host is a bare host name".into());
	let host = host.trim().to_ascii_lowercase();
	if host.is_empty() || host.len() > 253 || host.contains(['/', '@', ':', '?', '#', ' ', '*']) {
		return Err(invalid());
	}
	// Reuse the URL host grammar so that recorded and requested hosts agree.
	let parsed = url::Url::parse(&format!("https://{host}/")).map_err(|_| invalid())?;
	if parsed.host_str() != Some(host.as_str()) {
		return Err(invalid());
	}
	Ok(host)
}

/// Normalizes and de-duplicates a Tenant Secret's hosts.
pub fn normalize_hosts(hosts: &[String]) -> Result<Vec<String>> {
	if hosts.is_empty() || hosts.len() > MAX_HOSTS {
		return Err(Error::Invalid(format!(
			"a Tenant Secret records 1 to {MAX_HOSTS} hosts"
		)));
	}
	let mut normalized = Vec::with_capacity(hosts.len());
	for host in hosts {
		let host = normalize_host(host)?;
		if !normalized.contains(&host) {
			normalized.push(host);
		}
	}
	normalized.sort();
	Ok(normalized)
}

/// The host a tool or test endpoint would send a Tenant Secret to. A Tenant
/// Secret never travels in plaintext: the endpoint is HTTPS, or HTTP to a
/// literal loopback address.
pub fn endpoint_host(endpoint: &str) -> Result<String> {
	crate::configuration::validate_endpoint(endpoint)?;
	let url =
		url::Url::parse(endpoint).map_err(|_| Error::Invalid("invalid endpoint URL".into()))?;
	if url.scheme() != "https"
		&& !matches!(url.host(), Some(url::Host::Ipv4(address)) if address.is_loopback())
		&& !matches!(url.host(), Some(url::Host::Ipv6(address)) if address.is_loopback())
	{
		return Err(Error::Invalid(
			"a Tenant Secret endpoint requires HTTPS or a literal loopback HTTP address".into(),
		));
	}
	url.host_str()
		.map(str::to_ascii_lowercase)
		.ok_or_else(|| Error::Invalid("invalid endpoint URL".into()))
}

/// Whether a Tenant Secret with these recorded hosts may go to `endpoint`.
pub fn permits(hosts: &[String], endpoint: &str) -> Result<bool> {
	let host = endpoint_host(endpoint)?;
	Ok(hosts.contains(&host))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum State {
	/// The metadata exists before its value does, and expires if the write never finishes.
	Pending,
	Active,
	/// Deleted: the value is gone or being removed, and the name is free again.
	Deleted,
}

/// A Tenant Secret's metadata. The value is only ever in the Secret Store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantSecret {
	pub id: Uuid,
	pub tenant: String,
	pub name: String,
	/// The hosts this Tenant Secret may be sent to, sorted.
	pub hosts: Vec<String>,
	/// The user subject that owns a personal secret; `None` means shared with
	/// everyone in the Tenant (ADR 0100).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub owner: Option<String>,
	/// Adapter metadata, never part of an API projection.
	pub secret_resource: String,
	pub pinned_version: Option<String>,
	pub fingerprint: String,
	pub last4: String,
	pub state: State,
	pub created_at: DateTime<Utc>,
	/// When the value or the hosts last changed.
	pub updated_at: DateTime<Utc>,
	pub revision: i64,
}
impl TenantSecret {
	pub fn check_revision(&self, expected: i64) -> Result<()> {
		if self.revision != expected {
			return Err(Error::Conflict("stale Tenant Secret revision".into()));
		}
		Ok(())
	}
	pub fn require_active(&self) -> Result<&str> {
		match (self.state, self.pinned_version.as_deref()) {
			(State::Active, Some(version)) => Ok(version),
			_ => Err(Error::Conflict("Tenant Secret is not active".into())),
		}
	}
}

/// What a definition would send: one Tenant Secret to one host. Catalog
/// Approval shows these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "TenantSecretDisclosure")]
pub struct Disclosure {
	pub secret: String,
	pub host: String,
}

/// The secret a tool call resolves: the person's own wins; `SecretOwner::Any`
/// falls back to the shared Tenant Secret, `SecretOwner::Personal` never does.
pub fn pick_secret<'a>(
	personal: Option<&'a TenantSecret>,
	shared: Option<&'a TenantSecret>,
	owner: crate::tool::SecretOwner,
) -> Option<&'a TenantSecret> {
	match owner {
		crate::tool::SecretOwner::Any => personal.or(shared),
		crate::tool::SecretOwner::Personal => personal,
	}
}

/// The Tenant Secret a tool configuration names, with the endpoint it goes to.
pub fn named(config: &crate::tool::ToolConfig) -> Option<(&str, &str)> {
	match config {
		crate::tool::ToolConfig::Http {
			endpoint,
			secret: Some(secret),
			..
		}
		| crate::tool::ToolConfig::Mcp {
			endpoint,
			secret: Some(secret),
			..
		} => Some((secret, endpoint)),
		_ => None,
	}
}

/// The object of a tool definition's configuration that holds its HTTP or
/// MCP transport: the configuration itself for the legacy shape, the
/// `transport` member of a Tool descriptor otherwise.
fn transport_mut(
	config: &mut serde_json::Value,
) -> Option<&mut serde_json::Map<String, serde_json::Value>> {
	let legacy = config
		.get("transport")
		.is_some_and(serde_json::Value::is_string);
	let object = if legacy {
		config
	} else {
		config.get_mut("transport")?
	};
	let object = object.as_object_mut()?;
	matches!(
		object.get("transport").and_then(serde_json::Value::as_str),
		Some("http" | "mcp")
	)
	.then_some(object)
}

/// The Tenant Secret a Tool definition names, with the endpoint it goes to.
pub fn definition_target(entry: &crate::registry::Entry) -> Option<(String, String)> {
	if entry.kind == "external_service" {
		return Some((
			entry.config.get("secret")?.as_str()?.to_owned(),
			entry.config.get("base_url")?.as_str()?.to_owned(),
		));
	}
	if entry.kind != "tool" {
		return None;
	}
	let mut config = entry.config.clone();
	let object = transport_mut(&mut config)?;
	Some((
		object.get("secret")?.as_str()?.to_owned(),
		object.get("endpoint")?.as_str()?.to_owned(),
	))
}

/// The Tenant Secret a Tool definition names, if any.
pub fn definition_secret(entry: &crate::registry::Entry) -> Option<String> {
	definition_target(entry).map(|(secret, _)| secret)
}

/// Replaces the Tenant Secret a Tool definition names. Does nothing to a
/// definition that names none.
pub fn rename_definition_secret(entry: &mut crate::registry::Entry, name: &str) {
	if entry.kind == "external_service" && entry.config.get("secret").is_some() {
		entry.config["secret"] = name.into();
		return;
	}
	if entry.kind != "tool" {
		return;
	}
	if let Some(object) = transport_mut(&mut entry.config)
		&& object.contains_key("secret")
	{
		object.insert("secret".into(), name.into());
	}
}

#[cfg(test)]
mod tests;
