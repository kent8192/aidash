//! Runner HTTP retains TLS/loopback admission, rotating credentials and a ten-second timeout.
use aidash_application::{
	Error, Result,
	ports::{Credentials, capabilities::runner::RunnerTransport},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
pub struct RunnerHttp {
	pub endpoint: String,
	pub credential_env: String,
	pub credentials: Arc<dyn Credentials>,
}
#[async_trait]
impl RunnerTransport for RunnerHttp {
	async fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
		let url = reqwest::Url::parse(&self.endpoint)
			.map_err(|_| Error::Invalid("invalid runner endpoint".into()))?;
		if url.scheme() != "https"
			&& !(url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "::1")))
		{
			return Err(Error::Invalid(
				"runner transport requires TLS or loopback".into(),
			));
		}
		let token = self
			.credentials
			.resolve(&self.credential_env)
			.map_err(|_| {
				Error::Conflict("RUNTIME_UNAVAILABLE: runner credential unavailable".into())
			})?;
		let client = reqwest::Client::builder()
			.timeout(Duration::from_secs(10))
			.redirect(reqwest::redirect::Policy::none())
			.build()
			.map_err(crate::http_error)?;
		let method = reqwest::Method::from_bytes(method.as_bytes())
			.map_err(|_| Error::Invalid("invalid runner method".into()))?;
		let mut request = client
			.request(
				method,
				format!("{}{path}", self.endpoint.trim_end_matches('/')),
			)
			.bearer_auth(token);
		if let Some(body) = body {
			request = request.json(body);
		}
		let response = request.send().await.map_err(crate::http_error)?;
		let status = response.status();
		let value: Value = response.json().await.map_err(crate::http_error)?;
		if status == reqwest::StatusCode::NOT_FOUND && path.starts_with("/v1/operations/") {
			return Ok(json!({"status":"absent"}));
		}
		if !status.is_success() {
			return Err(Error::External(format!(
				"runner {}: {}",
				status.as_u16(),
				value["error"]
			)));
		}
		Ok(value)
	}
}
#[cfg(test)]
mod tests;
