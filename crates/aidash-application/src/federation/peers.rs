//! Peer identity admission and credential rotation use the same portable authority rules.
use crate::{
	Error, Result,
	ports::{
		Credentials,
		federation::peers::{PeerConfiguration, PeerIdentity},
	},
};
use aidash_domain::{
	configuration::{same_secret, validate_peer_endpoint},
	federation::{Peer, protocol::compatible},
	nodes::DeploymentId,
};
use std::sync::Arc;

pub struct PeerAuthority {
	pub node: String,
	pub protocol: String,
	pub configuration: Arc<dyn PeerConfiguration>,
	pub credentials: Arc<dyn Credentials>,
	pub identity: Arc<dyn PeerIdentity>,
}
impl PeerAuthority {
	pub async fn register(&self, peer: Peer) -> Result<Peer> {
		DeploymentId::parse(&peer.node_id)?;
		if peer.node_id == self.node || !compatible(&self.protocol, &peer.protocol_version) {
			return Err(Error::Invalid(format!(
				"peer must be another node with protocol_version {}",
				self.protocol
			)));
		}
		if peer.enabled {
			validate_peer_endpoint(&peer.endpoint)?;
		}
		let mut scope = self.configuration.write_scope().await?;
		if !peer.enabled {
			return scope.disable(&peer.node_id).await;
		}
		let credential = self.credentials.resolve(&peer.credential_env)?;
		let identity = self.identity.identity(&peer).await?;
		// Requests carry the registered version, so the peer's advertised
		// version must accept it; a newer peer still serves an older registration.
		let serves_registered = identity["protocol_version"]
			.as_str()
			.is_some_and(|advertised| compatible(advertised, &peer.protocol_version));
		if identity["id"] != peer.node_id || !serves_registered {
			return Err(Error::Invalid(
				"peer identity or protocol does not match".into(),
			));
		}
		scope.register(peer, &credential).await
	}
	pub async fn authenticate(&self, node: &str, supplied: &str) -> Result<()> {
		let peer = self.configuration.enabled(node).await?;
		let credential = self.credentials.resolve(&peer.credential_env)?;
		if !same_secret(supplied, &credential) {
			return Err(Error::Unauthorized);
		}
		for other in self
			.configuration
			.all()
			.await?
			.into_iter()
			.filter(|peer| peer.enabled && peer.node_id != node)
		{
			if self
				.credentials
				.resolve(&other.credential_env)
				.is_ok_and(|key| key == credential)
			{
				return Err(Error::Unauthorized);
			}
		}
		Ok(())
	}
}

/// Caller retains the registration lock while resolving each current credential.
pub fn require_distinct_credentials(
	peers: Vec<Peer>,
	node: &str,
	credential: &str,
	credentials: &dyn Credentials,
) -> Result<()> {
	for other in peers
		.into_iter()
		.filter(|peer| peer.enabled && peer.node_id != node)
	{
		if credentials.resolve(&other.credential_env)? == credential {
			return Err(Error::Invalid(
				"enabled peers must use distinct credentials for each node identity".into(),
			));
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests;
