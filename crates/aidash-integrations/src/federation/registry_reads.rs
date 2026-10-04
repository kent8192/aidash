//! Scoped verification keeps rotating credentials, a ten-second deadline and 1 KiB body bound.
use super::PeerHttp;
use aidash_application::ports::federation::registry_reads::{RegistryTransport, Verification};
use aidash_domain::federation::{Peer, registry_reads::Reference};
use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;
#[async_trait]
impl RegistryTransport for PeerHttp {
	async fn verify(
		&self,
		peer: &Peer,
		tenant: &str,
		subject: &str,
		references: &[Reference],
	) -> Verification {
		let Ok(token) = self.credentials.resolve(&peer.credential_env) else {
			return Verification::Rejected;
		};
		let response = self
			.client
			.post(format!(
				"{}/federation/v0.1/scoped/registry/verify",
				peer.endpoint.trim_end_matches('/')
			))
			.timeout(Duration::from_secs(10))
			.bearer_auth(token)
			.header("x-aidash-node", &self.node_id)
			.header("x-aidash-protocol", &self.protocol_version)
			.json(&json!({"tenant":tenant,"subject":subject,"references":references}))
			.send()
			.await;
		let response = match response {
			Ok(response) if response.status().is_success() => response,
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
