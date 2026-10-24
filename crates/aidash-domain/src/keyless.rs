//! Where a Keyless endpoint may connect (ADR 0036). A Keyless endpoint carries no
//! grant, so this rule is what keeps a Tenant's definition away from the
//! Deployment's private network.
use crate::{Error, Result, capabilities::outbound::public_ip};
use std::{collections::BTreeSet, net::IpAddr};

/// The private origins the Deployment Administrators permit for Keyless endpoints.
/// Entries are normalized origins (`scheme://host[:port]`), so a default port
/// such as `:80` for HTTP is omitted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PermittedOrigins(BTreeSet<String>);

impl PermittedOrigins {
	/// Each entry must be a bare HTTP(S) origin, without credentials, path,
	/// query or fragment.
	pub fn parse<I>(entries: I) -> Result<Self>
	where
		I: IntoIterator,
		I::Item: AsRef<str>,
	{
		entries
			.into_iter()
			.map(|entry| {
				let entry = entry.as_ref();
				let invalid = || {
					Error::Invalid(format!(
						"permitted private origin {entry:?} must be a bare HTTP(S) origin"
					))
				};
				let url = url::Url::parse(entry).map_err(|_| invalid())?;
				if !matches!(url.scheme(), "http" | "https")
					|| url.host_str().is_none()
					|| !url.username().is_empty()
					|| url.password().is_some()
					|| url.path() != "/"
					|| url.query().is_some()
					|| url.fragment().is_some()
				{
					return Err(invalid());
				}
				Ok(url.origin().ascii_serialization())
			})
			.collect::<Result<_>>()
			.map(Self)
	}

	pub fn permits(&self, origin: &str) -> bool {
		self.0.contains(origin)
	}

	/// The normalized origins, in order.
	pub fn iter(&self) -> impl Iterator<Item = &str> {
		self.0.iter().map(String::as_str)
	}

	/// Both sets of origins.
	pub fn with(mut self, other: Self) -> Self {
		self.0.extend(other.0);
		self
	}
}

/// How one Keyless endpoint may connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
	/// Its origin is permitted, so it may reach whatever address its host resolves to.
	Permitted,
	/// Only public addresses; every address its host resolves to is checked when connecting.
	PublicOnly,
	/// Its host is a non-public address and its origin is not permitted.
	Forbidden,
}

/// Decides how a Keyless endpoint may connect. A host that is an address
/// literal is decided here, because HTTP clients connect to it without
/// resolving a name and a connect-time check would never see it.
pub fn destination(endpoint: &str, permitted: &PermittedOrigins) -> Result<Destination> {
	crate::configuration::validate_endpoint(endpoint)?;
	let url =
		url::Url::parse(endpoint).map_err(|_| Error::Invalid("invalid endpoint URL".into()))?;
	if permitted.permits(&url.origin().ascii_serialization()) {
		return Ok(Destination::Permitted);
	}
	let literal = match url.host() {
		Some(url::Host::Ipv4(ip)) => Some(IpAddr::V4(ip)),
		Some(url::Host::Ipv6(ip)) => Some(IpAddr::V6(ip)),
		_ => None,
	};
	Ok(match literal {
		Some(ip) if !public_ip(ip) => Destination::Forbidden,
		_ => Destination::PublicOnly,
	})
}

#[cfg(test)]
mod tests;
