//! Keyless endpoints reach only public addresses unless the Deployment permits
//! their origin (ADR 0036). `provider_access` decides address literals and
//! permitted origins; this module checks host names against every address they
//! resolve to when a connection is made, so a later DNS answer cannot point a
//! checked name at a private address.
use crate::{Error, Result};
use aidash_application::provider_access::Access;
use aidash_domain::{capabilities::outbound::public_ip, keyless::Destination};
use std::{fmt, net::SocketAddr, sync::Arc, time::Duration};

/// The HTTP clients of model and embedding calls.
#[derive(Clone)]
pub struct Clients {
	/// Provider Catalog endpoints.
	pub direct: reqwest::Client,
	/// Keyless endpoints at an origin the Deployment permits. It follows no
	/// redirects, so the permitted origin cannot hand a request on to another
	/// private origin, port or scheme.
	pub permitted: reqwest::Client,
	/// Every other Keyless endpoint: public addresses only.
	pub public: reqwest::Client,
}

impl Clients {
	/// Pair `direct` with a public-only client using the given deadlines.
	pub fn new(
		direct: reqwest::Client,
		timeout: Duration,
		connect_timeout: Duration,
	) -> Result<Self> {
		Ok(Self {
			direct,
			permitted: reqwest::Client::builder()
				.timeout(timeout)
				.connect_timeout(connect_timeout)
				.redirect(reqwest::redirect::Policy::none())
				.build()
				.map_err(crate::http_error)?,
			public: public_client(timeout, connect_timeout)?,
		})
	}

	/// The client an access may use; a refused destination never reaches I/O.
	pub(crate) fn select(&self, access: &Access) -> Result<&reqwest::Client> {
		match access.keyless {
			None => Ok(&self.direct),
			Some(destination) => self.keyless(destination),
		}
	}

	/// The client a Keyless destination may use; a refused one never reaches I/O.
	pub fn keyless(&self, destination: Destination) -> Result<&reqwest::Client> {
		match destination {
			Destination::Permitted => Ok(&self.permitted),
			Destination::PublicOnly => Ok(&self.public),
			Destination::Forbidden => Err(Error::Forbidden),
		}
	}
}

/// A client whose every connection resolves only to public addresses. It
/// follows no redirects and uses no proxy, because a proxy resolves names
/// itself and would bypass the check.
pub fn public_client(timeout: Duration, connect_timeout: Duration) -> Result<reqwest::Client> {
	public_client_timeout(Some(timeout), connect_timeout)
}

pub(crate) fn public_client_timeout(
	timeout: Option<Duration>,
	connect_timeout: Duration,
) -> Result<reqwest::Client> {
	let builder = reqwest::Client::builder();
	let builder = if let Some(timeout) = timeout {
		builder.timeout(timeout)
	} else {
		builder
	};
	builder
		.connect_timeout(connect_timeout)
		.redirect(reqwest::redirect::Policy::none())
		.no_proxy()
		.dns_resolver(Arc::new(PublicDns))
		.build()
		.map_err(crate::http_error)
}

/// Map a failed send. A host that resolved to a non-public address is refused
/// as a configuration failure, never retried as a provider outage.
pub(crate) fn send_error(error: reqwest::Error) -> Error {
	if refused(&error) {
		tracing::warn!("endpoint refused: its host resolves to a non-public address");
		return Error::Forbidden;
	}
	crate::http_error(error)
}

/// Whether a send failed because the host resolved to a non-public address.
/// Brokers that forward Tenant Secret calls report that as a refused
/// destination rather than an upstream outage.
pub fn refused(error: &(dyn std::error::Error + 'static)) -> bool {
	let mut source = Some(error);
	while let Some(current) = source {
		if current.is::<NonPublicAddress>() {
			return true;
		}
		// `io::Error::source` skips the error it wraps, so look inside it.
		if let Some(inner) = current
			.downcast_ref::<std::io::Error>()
			.and_then(std::io::Error::get_ref)
			&& refused(inner)
		{
			return true;
		}
		source = current.source();
	}
	false
}

pub(crate) struct PublicDns;

impl reqwest::dns::Resolve for PublicDns {
	fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
		let host = name.as_str().to_owned();
		Box::pin(async move {
			// Port 0 is replaced by the URL's port or its scheme's default.
			let addresses: Vec<SocketAddr> =
				tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
			if addresses.is_empty() || addresses.iter().any(|address| !public_ip(address.ip())) {
				return Err(Box::new(NonPublicAddress) as Box<dyn std::error::Error + Send + Sync>);
			}
			Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
		})
	}
}

#[derive(Debug)]
struct NonPublicAddress;

impl fmt::Display for NonPublicAddress {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str("host resolves to a non-public address")
	}
}

impl std::error::Error for NonPublicAddress {}

#[cfg(test)]
mod tests;
