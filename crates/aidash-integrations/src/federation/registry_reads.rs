//! Scoped verification keeps a ten-second deadline and 1 KiB body bound.
use super::{HttpTransport, unavailable};
use aidash_application::{
	Error,
	ports::federation::registry_reads::{RegistryTransport, Verification},
};
use aidash_domain::federation::{registry_reads::Reference, route::Route};
use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;
#[async_trait]
impl RegistryTransport for HttpTransport {
	async fn verify(&self, route: &Route, references: &[Reference]) -> Verification {
		let response = tokio::time::timeout(
			Duration::from_secs(10),
			self.send(
				route,
				"POST",
				"/scoped/registry/verify",
				Some(&json!({"references":references})),
			),
		)
		.await;
		let response = match response {
			Ok(Ok(response)) if response.status().is_success() => response,
			// Nothing was sent: this Deployment refuses to federate.
			Ok(Err(Error::FederationDisabled)) => return Verification::Rejected,
			Ok(Ok(response)) => {
				let status = response.status();
				return if matches!(status.as_u16(), 401 | 403 | 404) {
					Verification::Rejected
				} else {
					unavailable(route, &format!("HTTP {status}"));
					Verification::Unavailable
				};
			}
			Ok(Err(error)) => {
				unavailable(route, &error);
				return Verification::Unavailable;
			}
			Err(_) => {
				unavailable(route, &"the request timed out");
				return Verification::Unavailable;
			}
		};
		match crate::response::json::<bool>(response, 1024).await {
			Ok(true) => Verification::Verified,
			Ok(false) => Verification::Rejected,
			Err(error) => {
				unavailable(route, &error);
				Verification::Unavailable
			}
		}
	}
}
#[cfg(test)]
mod tests;
