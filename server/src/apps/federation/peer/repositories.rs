//! Native peer registration retains its connection lease and transactional audit.
pub(crate) mod records;

use crate::federation::Federation;
use aidash_application::{
	Result,
	ports::federation::peers::{PeerConfiguration, PeerWrite},
};
use aidash_domain::{federation::Peer, registry::EntityRef};
use async_trait::async_trait;
use reinhardt::db::orm::connection::DatabaseConnectionLease;
use uuid::Uuid;

pub(crate) struct Configuration(pub(crate) Federation);
struct Write {
	records: records::PeerRecords,
	_lease: DatabaseConnectionLease,
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
		let lease = self.0.store.orm_connection()?;
		Ok(Box::new(Write {
			records: records::PeerRecords::new(lease.handle(), &self.0.config.node_id),
			_lease: lease,
		}))
	}
	async fn enabled(&self, node: &str) -> Result<Peer> {
		self.0.peer(node).await.map_err(Into::into)
	}
	async fn all(&self) -> Result<Vec<Peer>> {
		self.0.peers().await.map_err(Into::into)
	}
	async fn authorized(&self, node: &str, task: Uuid, agent: &EntityRef) -> Result<bool> {
		let lease = self.0.store.orm_connection()?;
		crate::apps::federation::remote::models::Delegation::authorized(
			&mut lease.handle(),
			task,
			node,
			agent,
		)
		.await
		.map_err(Into::into)
	}
}
