//! Immutable receiver admissions serialized by both source task and grant.
use super::AuthorizationRemoteAdmission;
use crate::Result;
use reinhardt::db::orm::{Model, OrmExecutor};

use uuid::Uuid;

impl AuthorizationRemoteAdmission {
	pub(crate) async fn by_id<E: OrmExecutor>(
		db: &mut E,
		id: Uuid,
		source: &str,
	) -> Result<Option<Self>> {
		Ok(Self::objects()
			.filter(Self::field_id().eq(id))
			.filter(Self::field_source_node().eq(source))
			.first_with_db(db)
			.await?)
	}
}
