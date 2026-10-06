//! HTTP and worker embedding callers share the application authority and accounting flow.
use crate::{Result, authorization::access::Access, semantic::EmbeddingConfig, store::Store};
use std::sync::Arc;
use uuid::Uuid;

pub(crate) use aidash_application::generation::embedding::{Origin, Reservation};

pub(crate) async fn reserve(
	access: &mut Access,
	store: &Store,
	workspace: Uuid,
	config: &EmbeddingConfig,
	text: &str,
	origin: Origin,
) -> Result<Option<Reservation>> {
	aidash_application::generation::embedding::reserve(
		&mut crate::bootstrap::generation_embedding_authority_scope(access),
		Arc::new(crate::bootstrap::generation_embedding_repository(store)),
		workspace,
		config,
		text,
		origin,
	)
	.await
	.map_err(Into::into)
}
