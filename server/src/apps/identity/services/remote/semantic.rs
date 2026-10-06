//! Home-side authority for explicitly disclosed remote semantic context.
use super::Inspection;
use crate::semantic::remote::{Operation, Receipt};
use crate::{
	Error, Result,
	authorization::access::Access,
	domain::Task,
	federation::Federation,
	semantic::remote::{Binding, Failure, Request},
};

use uuid::Uuid;

pub(crate) async fn binding(
	f: &Federation,
	access: &mut Access,
	task: &Task,
	node: &str,
	inspection: &Inspection,
	request: &Request,
) -> Result<Binding> {
	aidash_application::authorization::source::semantic::binding(
		&mut crate::bootstrap::source_semantic_binding_scope(f, access),
		task,
		node,
		inspection,
		request,
	)
	.await
	.map_err(Into::into)
}

pub(crate) fn failure(error: &Error) -> Failure {
	match error {
		Error::RemoteSemantic(reason) => *reason,
		Error::Forbidden | Error::Unauthorized => Failure::Authority,
		Error::Invalid(_) | Error::NotFound(_) => Failure::Configuration,
		_ => Failure::Unavailable,
	}
}

pub(crate) async fn search(
	f: Federation,
	headers: HeaderMap,
	operation: Operation,
) -> Result<Receipt> {
	let node = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	aidash_application::authorization::source::search::search(
		&crate::bootstrap::source_semantic_search_repository(&f),
		node,
		operation,
	)
	.await
	.map_err(Into::into)
}

impl Access {
	pub(crate) async fn remote_semantic_sources(&mut self, grant: Uuid) -> Result<()> {
		crate::apps::knowledge::repositories::remote_memory_reads::visible(self, grant).await?;
		aidash_application::authorization::source::provenance::verify(
			&mut crate::bootstrap::source_semantic_provenance_scope(self),
			grant,
		)
		.await
		.map_err(Into::into)
	}
}
use http::HeaderMap;
