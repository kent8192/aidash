//! Peer identity admission and credential rotation use the same portable authority rules.
use crate::{
	Error, Result,
	ports::{
		Credentials,
		federation::peers::{PeerConfiguration, PeerIdentity},
	},
};
use aidash_domain::{
	configuration::{same_secret, validate_endpoint, validate_node_id},
	federation::Peer,
	registry::EntityRef,
};
use std::sync::Arc;
use uuid::Uuid;

pub struct PeerAuthority {
	pub node: String,
	pub protocol: String,
	pub configuration: Arc<dyn PeerConfiguration>,
	pub credentials: Arc<dyn Credentials>,
	pub identity: Arc<dyn PeerIdentity>,
}
impl PeerAuthority {
	pub async fn register(&self, peer: Peer) -> Result<Peer> {
		validate_node_id(&peer.node_id)?;
		validate_endpoint(&peer.endpoint)?;
		if peer.node_id == self.node || peer.protocol_version != self.protocol {
			return Err(Error::Invalid(format!(
				"peer must be another node with protocol_version {}",
				self.protocol
			)));
		}
		let mut scope = self.configuration.write_scope().await?;
		if !peer.enabled {
			return scope.disable(&peer.node_id).await;
		}
		let credential = self.credentials.resolve(&peer.credential_env)?;
		let identity = self.identity.identity(&peer).await?;
		if identity["id"] != peer.node_id || identity["protocol_version"] != self.protocol {
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
	pub async fn authorize_task(&self, node: &str, task: Uuid, agent: &EntityRef) -> Result<()> {
		if !self.configuration.authorized(node, task, agent).await? {
			return Err(Error::Unauthorized);
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
