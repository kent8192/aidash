//! Bounded dependency RPCs use the same peer headers and rotating credentials.
use super::PeerHttp;
use aidash_application::ports::federation::dependencies::DependencyTransport;
use aidash_domain::federation::{
	Peer,
	dependencies::{Checked, Input},
};
use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;

#[async_trait]
impl DependencyTransport for PeerHttp {
	async fn check(&self, peer: &Peer, input: &Input, remaining: Duration) -> Option<Checked> {
		let token = self.credentials.resolve(&peer.credential_env).ok()?;
		let reply = self
			.client
			.post(format!(
				"{}/federation/v0.1/scoped/dependencies/verify",
				peer.endpoint.trim_end_matches('/')
			))
			.timeout(remaining.min(Duration::from_secs(10)))
			.bearer_auth(token)
			.header("x-aidash-node", &self.node_id)
			.header("x-aidash-protocol", &self.protocol_version)
			.json(
				&json!({"tenant":input.tenant,"subject":input.subject,"reference":input.reference}),
			)
			.send()
			.await
			.ok()?;
		if !reply.status().is_success() {
			return None;
		}
		crate::response::json(reply, 131072).await.ok()
	}
}

#[cfg(test)]
mod tests;
