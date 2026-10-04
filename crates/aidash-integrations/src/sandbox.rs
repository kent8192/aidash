//! Isolated HTTP test tools use no redirects, a bounded response, and a separate credential reference.
use aidash_application::{
	Result,
	ports::{
		Credentials,
		registry::workbench::sandbox::dispatch::{PreparedRealRequest, RealToolTransport},
	},
};
use aidash_domain::{provider::ToolCall, registry::workbench::profile::RealToolRule};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;
pub struct RealTools {
	credentials: Arc<dyn Credentials>,
}
impl RealTools {
	pub fn new(credentials: Arc<dyn Credentials>) -> Self {
		Self { credentials }
	}
}
struct Request(reqwest::RequestBuilder);
impl RealToolTransport for RealTools {
	fn prepare(
		&self,
		session: Uuid,
		rule: &RealToolRule,
		call: &ToolCall,
	) -> Result<Box<dyn PreparedRealRequest>> {
		let client = reqwest::Client::builder()
			.redirect(reqwest::redirect::Policy::none())
			.timeout(Duration::from_secs(30))
			.build()
			.map_err(crate::http_error)?;
		let mut request = client
			.post(&rule.endpoint)
			.header("idempotency-key", format!("test-{session}-{}", call.id))
			.json(&call.arguments);
		if let Some(name) = &rule.credential_env {
			request = request.bearer_auth(self.credentials.resolve(name)?);
		}
		Ok(Box::new(Request(request)))
	}
}
#[async_trait]
impl PreparedRealRequest for Request {
	async fn send(self: Box<Self>) -> Result<(Value, &'static str)> {
		let response = self.0.send().await.map_err(crate::http_error)?;
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
