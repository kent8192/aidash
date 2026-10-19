//! Native peer registration retains its connection lease and transactional audit.
pub(crate) mod records;

use crate::federation::Federation;
use aidash_application::{
	Result,
	ports::federation::peers::{PeerConfiguration, PeerWrite},
};
use aidash_domain::federation::Peer;
use async_trait::async_trait;

pub(crate) struct Configuration(pub(crate) Federation);
struct Write {
	records: records::PeerRecords,
}
#[async_trait]
impl PeerWrite for Write {
	async fn disable(&mut self, node: &str) -> Result<Peer> {
		self.records.disable(node).await.map_err(Into::into)
	}
	async fn register(&mut self, peer: Peer, credential: &str) -> Result<Peer> {
		self.records
			.register(peer, credential)
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl PeerConfiguration for Configuration {
	async fn write_scope(&self) -> Result<Box<dyn PeerWrite>> {
		Ok(Box::new(Write {
			records: records::PeerRecords::new(
				&self.0.store.pool,
				self.0.config.deployment.as_str(),
			),
		}))
	}
	async fn enabled(&self, node: &str) -> Result<Peer> {
		self.0.peer(node).await.map_err(Into::into)
	}
	async fn all(&self) -> Result<Vec<Peer>> {
		self.0.peers().await.map_err(Into::into)
	}
}
