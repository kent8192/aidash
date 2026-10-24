//! Local accounting is shared application logic; native code composes its ORM ports.
use crate::{Result, authorization::access::Access, provider::ModelResponse, store::Store};
pub(crate) use aidash_application::generation::inference::Reservation;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) async fn reserve(
	access: &mut Access,
	store: &Store,
	run: Uuid,
	attempt: Uuid,
	window: usize,
	output: u32,
) -> Result<Option<Reservation>> {
	let tenancy = access.tenancy()?;
	aidash_application::generation::inference::reserve(
		&mut crate::bootstrap::generation_inference_authority_scope(access),
		Arc::new(crate::bootstrap::generation_inference_repository(
			store, tenancy,
		)),
		run,
		attempt,
		window,
		output,
	)
	.await
	.map_err(Into::into)
}

pub(crate) async fn reserve_many(
	accesses: &mut [&mut Access],
	store: &Store,
	run: Uuid,
	attempt: Uuid,
	window: usize,
	output: u32,
) -> Result<Option<Reservation>> {
	use aidash_application::ports::generation::inference::GenerationInferenceAuthority;
	struct Union(Vec<Uuid>);
	#[async_trait::async_trait]
	impl GenerationInferenceAuthority for Union {
		async fn requests(&mut self, _: &str) -> aidash_application::Result<Vec<Uuid>> {
			Ok(self.0.clone())
		}
	}
	let mut requests = std::collections::BTreeSet::new();
	// Every lease of one Run belongs to the Run's Tenant.
	let tenancy = accesses.first().ok_or(crate::Error::Forbidden)?.tenancy()?;
	for access in accesses {
		access.resume_inherited().await?;
		let current = crate::bootstrap::generation_inference_authority_scope(access)
			.requests(store.deployment.as_str())
			.await;
		access.suspend().await?;
		requests.extend(current?);
	}
	aidash_application::generation::inference::reserve(
		&mut Union(requests.into_iter().collect()),
		Arc::new(crate::bootstrap::generation_inference_repository(
			store, tenancy,
		)),
		run,
		attempt,
		window,
		output,
	)
	.await
	.map_err(Into::into)
}
pub(crate) enum InferenceReservation {
	Local(Reservation),
	Remote(Box<super::remote::protocol::Reservation>),
}
impl InferenceReservation {
	pub(crate) async fn settle(self, response: &ModelResponse) -> Result<()> {
		match self {
			Self::Local(reservation) => reservation.settle(response).await.map_err(Into::into),
			Self::Remote(reservation) => reservation.settle(response).await,
		}
	}
	/// The request never left this node: refund a local charge whole. A remote
	/// Home aborts only a dispatch still preparing, so an admitted one settles
	/// with unknown usage instead of staying reserved.
	pub(crate) async fn abort(self) -> Result<()> {
		match self {
			Self::Local(reservation) => reservation.abort().await.map_err(Into::into),
			Self::Remote(reservation) => reservation.abort().await,
		}
	}
}
