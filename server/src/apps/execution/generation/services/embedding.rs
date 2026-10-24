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
	let tenancy = access.tenancy()?;
	aidash_application::generation::embedding::reserve(
		&mut crate::bootstrap::generation_embedding_authority_scope(access),
		Arc::new(crate::bootstrap::generation_embedding_repository(
			store, tenancy,
		)),
		workspace,
		config,
		text,
		origin,
	)
	.await
	.map_err(Into::into)
}

/// `tenancy` is the scope of the reservation: the Tenant whose bank or Run
/// asks for the embedding, which holds even when no origin lease applies.
pub(crate) async fn reserve_many(
	accesses: &mut [&mut Access],
	store: &Store,
	tenancy: aidash_application::tenancy::TenancyScope,
	workspace: Uuid,
	config: &EmbeddingConfig,
	text: &str,
	origin: Origin,
) -> Result<Option<Reservation>> {
	let mut approved = vec![];
	for access in accesses {
		access.resume_inherited().await?;
		let current = aidash_application::generation::embedding::authorize(
			&mut crate::bootstrap::generation_embedding_authority_scope(access),
			store.deployment.as_str(),
			config,
		)
		.await;
		access.suspend().await?;
		if let Some(origin) = current? {
			approved.push(origin);
		}
	}
	aidash_application::generation::embedding::reserve_approved(
		approved,
		Arc::new(crate::bootstrap::generation_embedding_repository(
			store, tenancy,
		)),
		workspace,
		text,
		origin,
	)
	.await
	.map_err(Into::into)
}
