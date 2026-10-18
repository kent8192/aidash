//! A Tenant's Peer Mappings, which say who outside the Deployment acts with
//! which of its credentials (#359).
use super::{TenantAdministrator, rules};
use crate::Result;
use crate::apps::federation::peer::models::{
	AuthorizationPeerMapping, AuthorizationPeerMappingHistory,
};
use crate::apps::identity::serializers::peer::{MappingRevision, PeerMapping};
use crate::database::native::Pool;

impl TenantAdministrator {
	pub(crate) async fn peer_mappings(
		&self,
		db: &Pool,
		offset: usize,
		limit: usize,
	) -> Result<Vec<PeerMapping>> {
		db.atomic(&self.scope()?, async |tx| {
			self.locked_for(tx, rules::PEER_MAPPING_READ, false).await?;
			Ok(
				AuthorizationPeerMapping::page(tx, self.tenant(), offset, limit)
					.await?
					.into_iter()
					.map(PeerMapping::from)
					.collect(),
			)
		})
		.await
	}

	pub(crate) async fn peer_mapping_history(
		&self,
		db: &Pool,
		after: i64,
		limit: usize,
	) -> Result<Vec<MappingRevision>> {
		db.atomic(&self.scope()?, async |tx| {
			self.locked_for(tx, rules::PEER_MAPPING_READ, false).await?;
			Ok(
				AuthorizationPeerMappingHistory::page(tx, self.tenant(), after, limit)
					.await?
					.into_iter()
					.map(MappingRevision::from)
					.collect(),
			)
		})
		.await
	}
}
