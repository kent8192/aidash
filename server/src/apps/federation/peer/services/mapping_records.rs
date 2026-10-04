//! Read peer mapping pages through the request's injected ORM connection.
use crate::Result;
use crate::apps::federation::peer::models::{
	AuthorizationPeerMapping, AuthorizationPeerMappingHistory,
};
use crate::apps::identity::serializers::peer::{MappingRevision, PeerMapping};
use reinhardt::db::orm::DatabaseConnection;
use reinhardt::injectable;

#[derive(Clone)]
pub(crate) struct PeerMappingRecords {
	connection: DatabaseConnection,
}

#[injectable(scope = "request")]
pub(crate) async fn provide(#[inject] connection: DatabaseConnection) -> PeerMappingRecords {
	PeerMappingRecords { connection }
}

impl PeerMappingRecords {
	pub(crate) async fn history(
		&self,
		tenant: &str,
		after: i64,
		limit: usize,
	) -> Result<Vec<MappingRevision>> {
		let mut connection = self.connection;
		Ok(
			AuthorizationPeerMappingHistory::page(&mut connection, tenant, after, limit)
				.await?
				.into_iter()
				.map(MappingRevision::from)
				.collect(),
		)
	}

	pub(crate) async fn list(
		&self,
		tenant: &str,
		offset: usize,
		limit: usize,
	) -> Result<Vec<PeerMapping>> {
		let mut connection = self.connection;
		Ok(
			AuthorizationPeerMapping::page(&mut connection, tenant, offset, limit)
				.await?
				.into_iter()
				.map(PeerMapping::from)
				.collect(),
		)
	}
}
