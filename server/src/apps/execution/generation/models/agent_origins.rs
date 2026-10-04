//! Provenance lookup for registry discovery.
use super::GenerationRequest;
use crate::Result;
use reinhardt::db::orm::{DatabaseConnection, Model};

pub(crate) async fn is_generated_agent(
	mut db: DatabaseConnection,
	id: &str,
	version: &str,
) -> Result<bool> {
	Ok(GenerationRequest::objects()
		.filter(GenerationRequest::field_agent_id().eq(id.to_owned()))
		.filter(GenerationRequest::field_agent_version().eq(version.to_owned()))
		.exists_with_db(&mut db)
		.await?)
}
