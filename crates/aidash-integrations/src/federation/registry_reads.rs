//! Scoped verification keeps a ten-second deadline and 1 KiB body bound.
use super::HttpTransport;
use aidash_application::ports::federation::registry_reads::{RegistryTransport, Verification};
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
			_ => return Verification::Unavailable,
		};
		if crate::response::json::<bool>(response, 1024)
			.await
			.unwrap_or(false)
		{
			Verification::Verified
		} else {
			Verification::Rejected
		}
	}
}
#[cfg(test)]
mod tests;
