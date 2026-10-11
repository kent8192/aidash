//! Provider packages of the host-owned research contract. Each implements a
//! research port; the host owns validation, evidence and citations.
pub mod brave;
pub mod fixture;
pub mod page;

use aidash_domain::{capabilities::outbound::public_ip, research::Failure};
use std::net::IpAddr;
use url::{Host, Url};

/// Literal destination checks shared by every page provider: http(s) on a
/// standard port, no credentials, and no name or address that is non-public
/// by construction. Resolved addresses are checked again at dispatch.
pub fn admit_public_url(url: &Url) -> Result<(), Failure> {
	if !matches!(url.scheme(), "http" | "https")
		|| !url.username().is_empty()
		|| url.password().is_some()
		|| !matches!(url.port_or_known_default(), Some(80 | 443))
	{
		return Err(Failure::NonPublicDestination);
	}
	match url.host() {
		Some(Host::Ipv4(ip)) if public_ip(IpAddr::V4(ip)) => Ok(()),
		Some(Host::Ipv6(ip)) if public_ip(IpAddr::V6(ip)) => Ok(()),
		Some(Host::Domain(name)) => {
			let name = name.trim_end_matches('.').to_ascii_lowercase();
			let local = ["localhost", "local", "internal", "localdomain", "home.arpa"]
				.iter()
				.any(|suffix| name == *suffix || name.ends_with(&format!(".{suffix}")));
			if local || !name.contains('.') {
				Err(Failure::NonPublicDestination)
			} else {
				Ok(())
			}
		}
		_ => Err(Failure::NonPublicDestination),
	}
}

#[cfg(test)]
mod tests;
