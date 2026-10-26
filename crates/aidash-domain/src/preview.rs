//! Previews: the random origin one File version or Live preview is served from,
//! the one-time ticket that opens it, and the response policy that confines it
//! (ADR 0068, ADR 0069).
//!
//! A Preview is addressed by a random subdomain of the Deployment's preview
//! domain, so every Preview is its own site. The dashboard authorizes a view
//! exactly as it authorizes a download, then a one-time ticket is exchanged by
//! the Preview gateway for a host-only cookie on that subdomain.
use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// The host-only cookie the Preview gateway sets on a Preview's subdomain.
/// The `__Host-` prefix requires `Secure`, `Path=/` and no `Domain`.
pub const COOKIE: &str = "__Host-aidash-preview";
/// The path the gateway exchanges a ticket on.
pub const TICKET_PATH: &str = "/__aidash/ticket";
/// A ticket is valid for one minute.
pub const TICKET_SECONDS: i64 = 60;
/// A viewing session lasts an hour; a new ticket starts another.
pub const SESSION_SECONDS: i64 = 3600;
/// A Preview subdomain carries at least 128 random bits.
const LABEL_BYTES: usize = 17;
/// The shortest valid label: 26 base32 characters encode 130 bits.
const LABEL_MIN: usize = 26;
const LABEL_MAX: usize = 63;

/// What a Preview shows: one File version, or (later) one Live preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PreviewKind {
	File { area_id: Uuid, file_id: Uuid },
	Live { live_preview_id: Uuid },
}

impl PreviewKind {
	pub fn name(&self) -> &'static str {
		match self {
			Self::File { .. } => "file",
			Self::Live { .. } => "live",
		}
	}
}

/// The random DNS label under the preview domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct PreviewLabel(String);

impl PreviewLabel {
	/// A label from at least 128 bits: 17 random bytes in lowercase base32.
	pub fn generate() -> Result<Self> {
		let mut bytes = [0_u8; LABEL_BYTES];
		getrandom::fill(&mut bytes)
			.map_err(|_| Error::Invalid("preview label entropy unavailable".into()))?;
		Ok(Self(base32_lower(&bytes)))
	}

	pub fn parse(value: impl Into<String>) -> Result<Self> {
		let value = value.into();
		if value.len() < LABEL_MIN
			|| value.len() > LABEL_MAX
			|| !value
				.bytes()
				.all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
		{
			return Err(Error::Invalid("invalid preview label".into()));
		}
		Ok(Self(value))
	}

	pub fn as_str(&self) -> &str {
		&self.0
	}

	/// `<label>.<preview_domain>`, the only Host this Preview answers on.
	pub fn host(&self, domain: &str) -> String {
		format!("{}.{domain}", self.0)
	}
}

/// RFC 4648 base32, lowercase and unpadded, which is a DNS-safe alphabet.
fn base32_lower(bytes: &[u8]) -> String {
	const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
	let mut output = String::with_capacity(bytes.len().div_ceil(5) * 8);
	let mut buffer = 0_u32;
	let mut bits = 0_u32;
	for byte in bytes {
		buffer = (buffer << 8) | u32::from(*byte);
		bits += 8;
		while bits >= 5 {
			bits -= 5;
			output.push(ALPHABET[((buffer >> bits) & 0x1f) as usize] as char);
		}
	}
	if bits > 0 {
		output.push(ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize] as char);
	}
	output
}

/// A one-time ticket secret, or the value of a viewing session cookie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewSecret(String);

impl PreviewSecret {
	/// 256 random bits in URL-safe base64.
	pub fn generate() -> Result<Self> {
		let mut bytes = [0_u8; 32];
		getrandom::fill(&mut bytes)
			.map_err(|_| Error::Invalid("preview ticket entropy unavailable".into()))?;
		Ok(Self(URL_SAFE_NO_PAD.encode(bytes)))
	}

	pub fn parse(value: impl Into<String>) -> Result<Self> {
		let value = value.into();
		if value.len() < 32 || value.len() > 128 {
			return Err(Error::Invalid("invalid preview secret".into()));
		}
		Ok(Self(value))
	}

	pub fn expose(&self) -> &str {
		&self.0
	}

	/// What the database stores; the secret itself never reaches it.
	pub fn digest(&self) -> String {
		format!("{:x}", Sha256::digest(self.0.as_bytes()))
	}
}

/// The persisted facts a Ticket must satisfy at exchange time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
	pub expires_at: DateTime<Utc>,
	pub used_at: Option<DateTime<Utc>>,
	pub subject: String,
	pub origin: String,
}

impl Ticket {
	/// A ticket is single use, expires, and is bound to its subject and to the
	/// dashboard origin that requested it.
	pub fn accept(&self, now: DateTime<Utc>, subject: &str, origin: &str) -> Result<()> {
		if self.used_at.is_some() {
			return Err(Error::Conflict("preview ticket already used".into()));
		}
		if now >= self.expires_at {
			return Err(Error::Conflict("preview ticket expired".into()));
		}
		if self.subject != subject || self.origin != origin {
			return Err(Error::Conflict(
				"preview ticket is bound to another viewer".into(),
			));
		}
		Ok(())
	}
}

/// A preview domain is a bare registrable domain, or `localhost` for a local
/// cluster whose previews live under `*.localhost`.
pub fn preview_domain(value: &str) -> Result<String> {
	let value = value.trim().to_ascii_lowercase();
	if value.is_empty()
		|| value.contains(['/', ':', '@', '*', '?', '#'])
		|| value.starts_with('.')
		|| value.ends_with('.')
		|| value.split('.').any(|label| {
			label.is_empty()
				|| label.len() > 63
				|| label.starts_with('-')
				|| label.ends_with('-')
				|| !label
					.bytes()
					.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
		}) {
		return Err(Error::Invalid("invalid preview domain".into()));
	}
	if value == "localhost" {
		return Ok(value);
	}
	if value.parse::<std::net::IpAddr>().is_ok() || psl::domain_str(&value) != Some(value.as_str())
	{
		return Err(Error::Invalid(
			"preview domain must be a registrable domain".into(),
		));
	}
	Ok(value)
}

/// The dashboard's origin, canonicalized to `scheme://host[:port]`.
pub fn origin(value: &str) -> Result<String> {
	let url =
		url::Url::parse(value).map_err(|_| Error::Invalid("invalid preview origin".into()))?;
	if !matches!(url.scheme(), "http" | "https")
		|| url.host_str().is_none()
		|| !url.username().is_empty()
		|| url.password().is_some()
		|| url.path() != "/"
		|| url.query().is_some()
		|| url.fragment().is_some()
	{
		return Err(Error::Invalid("invalid preview origin".into()));
	}
	Ok(url.origin().ascii_serialization())
}

/// A Deployment refuses to start when its preview domain and its dashboard's
/// public origin share a registrable domain: previews must never be same-site
/// with the dashboard (ADR 0068). `localhost` previews are the local exception.
pub fn refuse_shared_registrable_domain(preview: &str, dashboard_origin: &str) -> Result<()> {
	let preview = preview_domain(preview)?;
	let dashboard = url::Url::parse(dashboard_origin)
		.map_err(|_| Error::Invalid("invalid dashboard origin".into()))?;
	let dashboard = dashboard
		.host_str()
		.ok_or_else(|| Error::Invalid("invalid dashboard origin".into()))?
		.to_ascii_lowercase();
	if preview == "localhost" {
		let local = matches!(dashboard.as_str(), "localhost" | "127.0.0.1" | "[::1]")
			|| dashboard.ends_with(".localhost");
		return if local || dashboard.parse::<std::net::IpAddr>().is_ok() {
			Ok(())
		} else {
			Err(Error::Invalid(
				"a localhost preview domain requires a local dashboard origin".into(),
			))
		};
	}
	if let Some(shared) = psl::domain_str(&dashboard)
		&& psl::domain_str(&preview) == Some(shared)
	{
		return Err(Error::Invalid(
			"preview domain and dashboard origin share a registrable domain".into(),
		));
	}
	Ok(())
}

/// The Content Security Policy every Preview response carries: the page may
/// read and run only its own origin, and only the requesting dashboard origin
/// may frame it (ADR 0069).
pub fn content_security_policy(frame_ancestor: &str) -> Result<String> {
	let frame_ancestor = origin(frame_ancestor)?;
	Ok(format!(
		"default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self'; connect-src 'self'; media-src 'self'; worker-src 'self' blob:; frame-ancestors {frame_ancestor}; base-uri 'self'; form-action 'self'; object-src 'none'"
	))
}

/// The `Host` header of a Preview request, without its port.
pub fn request_host(host: &str) -> Option<&str> {
	let host = host.trim();
	let host = host.strip_prefix('[').map_or(host, |rest| {
		rest.split_once(']').map_or(rest, |(address, _)| address)
	});
	let (name, port) = match host.rsplit_once(':') {
		Some((name, port)) if !name.contains(':') => (name, Some(port)),
		_ => (host, None),
	};
	if let Some(port) = port
		&& port.parse::<u16>().is_err()
	{
		return None;
	}
	(!name.is_empty()).then_some(name)
}

/// Split a Preview request's Host into its label and the configured domain.
pub fn label_of<'a>(host: &'a str, domain: &str) -> Option<&'a str> {
	let host = request_host(host)?;
	let label = host.strip_suffix(domain)?.strip_suffix('.')?;
	(!label.is_empty() && !label.contains('.')).then_some(label)
}

#[cfg(test)]
mod tests;
