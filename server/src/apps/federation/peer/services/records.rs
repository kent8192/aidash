//! Request injection exposes the native peer repository through its compatibility API.
pub(crate) use crate::apps::federation::peer::repositories::records::PeerRecords;
use crate::federation::Federation;
use reinhardt::{db::orm::DatabaseConnection, injectable};

#[injectable(scope = "request")]
pub(crate) async fn provide(
	#[inject] db: DatabaseConnection,
	#[inject] runtime: Federation,
) -> PeerRecords {
	PeerRecords::new(db, &runtime.config.node_id)
}
