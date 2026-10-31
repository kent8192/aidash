//! The clients that reach an endpoint a Tenant Administrator typed (ADR 0036,
//! ADR 0064). Public addresses only, vetted when each connection is made, so a
//! later DNS answer cannot point a checked name at a private address, unless
//! the Deployment permits the endpoint's origin. No redirects, no proxy.
use crate::{Result, keyless::public_client};
use aidash_application::{Error, provider_access::KeylessAccess};
use aidash_domain::keyless::Destination;
use std::time::Duration;

#[derive(Clone)]
pub struct Endpoints {
	access: KeylessAccess,
	/// A permitted private origin: no redirects, so it cannot hand the request
	/// on to another private origin.
	permitted: reqwest::Client,
	/// Public addresses only.
	public: reqwest::Client,
}

/// The two clients, built once per process: a client owns a connection pool.
static CLIENTS: std::sync::OnceLock<(reqwest::Client, reqwest::Client)> =
	std::sync::OnceLock::new();

impl Endpoints {
	/// `access` holds the origins the Deployment permits.
	pub fn new(access: KeylessAccess) -> Result<Self> {
		let timeout = Duration::from_secs(10);
		let (permitted, public) = match CLIENTS.get() {
			Some(clients) => clients.clone(),
			None => {
				let clients = (
					reqwest::Client::builder()
						.connect_timeout(timeout)
						.redirect(reqwest::redirect::Policy::none())
						.user_agent(crate::USER_AGENT)
						.no_proxy()
						.build()
						.map_err(crate::http_error)?,
					public_client(Duration::from_secs(120), timeout)?,
				);
				CLIENTS.get_or_init(|| clients).clone()
			}
		};
		Ok(Self {
			access,
			permitted,
			public,
		})
	}

	/// The client that may reach `endpoint`. A public endpoint must use https.
	pub fn client(&self, endpoint: &str) -> Result<&reqwest::Client> {
		match self.access.destination(endpoint)? {
			Destination::Permitted => Ok(&self.permitted),
			Destination::PublicOnly if endpoint.starts_with("https://") => Ok(&self.public),
			Destination::PublicOnly => {
				Err(Error::Invalid("a public endpoint must use https".into()))
			}
			Destination::Forbidden => Err(Error::Forbidden),
		}
	}
}
