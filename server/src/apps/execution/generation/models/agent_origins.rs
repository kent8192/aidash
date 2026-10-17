//! Provenance lookup for registry discovery.
use super::GenerationRequest;
use crate::Result;
use reinhardt::db::orm::{Model, OrmExecutor};

pub(crate) async fn is_generated_agent<E: OrmExecutor>(
	db: &mut E,
	id: &str,
	version: &str,
) -> Result<bool> {
	Ok(GenerationRequest::objects()
		.filter(GenerationRequest::field_agent_id().eq(id.to_owned()))
		.filter(GenerationRequest::field_agent_version().eq(version.to_owned()))
		.exists_with_db(db)
		.await?)
}
