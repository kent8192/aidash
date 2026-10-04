//! Peer configuration and its audit event share a native ORM transaction.
use crate::apps::{execution::models::event_records, federation::peer::models::records};
use crate::{Result, federation::Peer};
use reinhardt::db::orm::DatabaseConnection;
use serde_json::json;

#[derive(Clone)]
pub(crate) struct PeerRecords {
	db: DatabaseConnection,
	node: String,
}

impl PeerRecords {
	pub(crate) fn new(db: DatabaseConnection, node: &str) -> Self {
		Self {
			db,
			node: node.to_owned(),
		}
	}
	pub(crate) async fn list(&self) -> Result<Vec<Peer>> {
		let mut db = self.db;
		records::list(&mut db).await
	}
	pub(crate) async fn enabled(&self, node: &str) -> Result<Peer> {
		let mut db = self.db;
		records::enabled(&mut db, node).await
	}
	pub(crate) async fn disable(&self, node: &str) -> Result<Peer> {
		self.db
			.atomic(async |tx| {
				let stored = records::disable(tx, node).await?;
				event_records::append(
					tx,
					&self.node,
					None,
					"peer.registered",
					json!({"node_id":stored.node_id,"endpoint":stored.endpoint,"enabled":false}),
				)
				.await?;
				Ok(stored)
			})
			.await
	}
	pub(crate) async fn register(&self, peer: Peer, credential: &str) -> Result<Peer> {
		self.db
			.atomic(async |tx| {
				records::lock_registration(tx).await?;
				aidash_application::federation::peers::require_distinct_credentials(
					records::list(tx).await?,
					&peer.node_id,
					credential,
					&crate::bootstrap::peer_credentials(),
				)?;
				records::save(tx, &peer).await?;
				event_records::append(
					tx,
					&self.node,
					None,
					"peer.registered",
					json!({"node_id":peer.node_id,"endpoint":peer.endpoint,"enabled":peer.enabled}),
				)
				.await?;
				Ok(peer)
			})
			.await
	}
}
