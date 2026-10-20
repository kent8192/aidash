//! Peer configuration and its audit event share a native ORM transaction.
use crate::apps::{execution::models::event_records, federation::peer::models::records};
use crate::database::native::Pool;
use crate::{Result, federation::Peer};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use serde_json::json;

/// Peer configuration has no Tenant until #241. Reading it resolves which
/// peer a request came from; changing it is a Deployment Administrator's work.
fn reading() -> TenancyScope {
	TenancyScope::Deployment(DeploymentScopeReason::FederationInbound)
}
fn administering() -> TenancyScope {
	TenancyScope::Deployment(DeploymentScopeReason::DeploymentAdministration)
}

#[derive(Clone)]
pub(crate) struct PeerRecords {
	db: Pool,
}

impl PeerRecords {
	pub(crate) fn new(db: &Pool) -> Self {
		Self { db: db.clone() }
	}
	pub(crate) async fn list(&self) -> Result<Vec<Peer>> {
		self.db
			.atomic(&reading(), async |tx| records::list(tx).await)
			.await
	}
	/// Peers are registered per Deployment until Node Registrations replace them,
	/// so `node` may be the address of a Node of that Deployment.
	pub(crate) async fn enabled(&self, node: &str) -> Result<Peer> {
		let deployment = aidash_domain::nodes::deployment_of(node);
		self.db
			.atomic(&reading(), async |tx| {
				records::enabled(tx, deployment).await
			})
			.await
	}
	pub(crate) async fn disable(&self, node: &str) -> Result<Peer> {
		self.db
			.atomic(&administering(), async |tx| {
				let stored = records::disable(tx, node).await?;
				event_records::append(
					tx,
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
			.atomic(&administering(), async |tx| {
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
