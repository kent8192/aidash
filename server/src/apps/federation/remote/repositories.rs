//! Native adapters for Deployment Administrator Agent discovery across peers.
use aidash_application::{Result, ports::federation::FederationRepository};
use aidash_domain::federation::Peer;
use async_trait::async_trait;

pub(crate) struct Repository {
	pub federation: crate::federation::Federation,
}
#[async_trait]
impl FederationRepository for Repository {
	async fn peers(&self) -> Result<Vec<Peer>> {
		self.federation.peers().await.map_err(Into::into)
	}
	async fn peer(&self, node: &str) -> Result<Peer> {
		self.federation.peer(node).await.map_err(Into::into)
	}
}

#[async_trait]
impl aidash_application::ports::federation::authority::Peers for Repository {
	async fn peer(&self, node: &str) -> Result<Peer> {
		self.federation.peer(node).await.map_err(Into::into)
	}
}

pub(crate) mod run_messages;

pub(crate) mod headroom;
