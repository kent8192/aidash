//! Isolated HTTP test tools use no redirects and a bounded response. A test
//! endpoint is the Tenant's choice, so it reaches public addresses only,
//! unless the Deployment permits its origin (#361, ADR 0036).
use aidash_application::{
	Result,
	ports::registry::workbench::sandbox::dispatch::{PreparedRealRequest, RealToolTransport},
	provider_access::KeylessAccess,
};
use aidash_domain::{
	keyless::Destination, provider::ToolCall, registry::workbench::profile::RealToolRule,
};
use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;
const TIMEOUT: Duration = Duration::from_secs(30);
pub struct RealTools {
	origins: KeylessAccess,
	/// For a permitted origin. It follows no redirects, so the origin cannot
	/// hand a request on to another private origin.
	permitted: reqwest::Client,
	/// Every other endpoint: public addresses only.
	public: reqwest::Client,
}
impl RealTools {
	pub fn new(origins: KeylessAccess) -> crate::Result<Self> {
		Ok(Self {
			origins,
			permitted: reqwest::Client::builder()
				.redirect(reqwest::redirect::Policy::none())
				.user_agent(crate::USER_AGENT)
				.timeout(TIMEOUT)
				.build()
				.map_err(crate::http_error)?,
			public: crate::keyless::public_client(TIMEOUT, TIMEOUT)?,
		})
	}
}
struct Request(reqwest::RequestBuilder);
impl RealToolTransport for RealTools {
	fn prepare(
		&self,
		session: Uuid,
		rule: &RealToolRule,
		call: &ToolCall,
		secret: Option<&SecretString>,
	) -> Result<Box<dyn PreparedRealRequest>> {
		let client = match self.origins.destination(&rule.endpoint)? {
			Destination::Permitted => &self.permitted,
			Destination::PublicOnly => &self.public,
			Destination::Forbidden => return Err(aidash_application::Error::Forbidden),
		};
		let mut request = client
			.post(&rule.endpoint)
			.header("idempotency-key", format!("test-{session}-{}", call.id))
			.json(&call.arguments);
		// A Tenant Secret goes only to the test endpoint, one of its recorded hosts.
		if let Some(secret) = secret {
			request = request.bearer_auth(secret.expose_secret());
		}
		Ok(Box::new(Request(request)))
	}
}
#[async_trait]
impl PreparedRealRequest for Request {
	async fn send(self: Box<Self>) -> Result<(Value, &'static str)> {
		let response = self.0.send().await.map_err(crate::keyless::send_error)?;
		if response.status().is_success() {
			Ok((crate::response::json(response, 256_000).await?, "real"))
		} else {
			Ok((
				json!({"error":"test Tool returned an HTTP error","status":response.status().as_u16()}),
				"failed",
			))
		}
	}
}
