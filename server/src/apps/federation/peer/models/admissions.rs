//! Receiver admissions serialized by both source task and grant. What an admission binds never
//! changes; only the Home's Federation Tool Request it keeps follows the Home's latest describe
//! (ADR 0139), besides its cancellation and the receiving Node's Run-only answers.
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
