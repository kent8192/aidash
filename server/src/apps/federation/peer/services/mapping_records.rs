//! Read peer mapping pages through the request's injected ORM connection.
use crate::Result;
use crate::apps::federation::peer::models::{
	AuthorizationPeerMapping, AuthorizationPeerMappingHistory,
};
use crate::apps::identity::serializers::peer::{MappingRevision, PeerMapping};
use crate::database::native::Pool;
use crate::federation::Federation;
use aidash_application::tenancy::TenancyScope;
use reinhardt::injectable;

#[derive(Clone)]
pub(crate) struct PeerMappingRecords {
	pool: Pool,
}

#[injectable(scope = "request")]
pub(crate) async fn provide(#[inject] runtime: Federation) -> PeerMappingRecords {
	PeerMappingRecords {
		pool: runtime.store.pool.clone(),
	}
}

impl PeerMappingRecords {
	pub(crate) async fn history(
		&self,
		tenant: &str,
		after: i64,
		limit: usize,
	) -> Result<Vec<MappingRevision>> {
		Ok(self
			.pool
			.atomic(&TenancyScope::tenant(tenant)?, async |tx| {
				AuthorizationPeerMappingHistory::page(tx, tenant, after, limit).await
			})
			.await?
			.into_iter()
			.map(MappingRevision::from)
			.collect())
	}

	pub(crate) async fn list(
		&self,
		tenant: &str,
		offset: usize,
		limit: usize,
	) -> Result<Vec<PeerMapping>> {
		Ok(self
			.pool
			.atomic(&TenancyScope::tenant(tenant)?, async |tx| {
				AuthorizationPeerMapping::page(tx, tenant, offset, limit).await
			})
			.await?
			.into_iter()
			.map(PeerMapping::from)
			.collect())
	}
}
