//! Local quota and Home protocol adapters retain the same worker scope and reservation types.
use crate::{
	Error, Result as NativeResult,
	apps::identity::services::access::Access,
	domain::Run,
	federation::Federation,
	generation::{
		budget::InferenceReservation,
		remote::{Purpose, protocol},
	},
	store::Store,
};
use aidash_application::{Result, ports::execution::admission::InferenceAdmissionRepository};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;
pub(crate) struct Admissions<'a> {
	pub(crate) store: &'a Store,
	pub(crate) remote: Option<&'a Federation>,
	pub(crate) access: &'a Arc<RwLock<Access>>,
	pub(crate) run: &'a Run,
}
#[async_trait]
impl InferenceAdmissionRepository<InferenceReservation> for Admissions<'_> {
	fn remote(&self) -> bool {
		self.remote.is_some()
	}
	async fn suspend(&self) -> Result<()> {
		let result: NativeResult<()> = async {
			let mut access = self.access.write().await;
			if access.tx.is_active() {
				access.suspend().await?;
			}
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn admit_remote(
		&self,
		attempt: Uuid,
		digest: String,
		units: i64,
	) -> Result<InferenceReservation> {
		let federation = self.remote.ok_or(Error::Forbidden)?;
		protocol::admit(
			federation,
			self.run,
			attempt,
			Purpose::Inference,
			digest,
			units,
		)
		.await
		.map(|reservation| InferenceReservation::Remote(Box::new(reservation)))
		.map_err(Into::into)
	}
	async fn reserve_local(
		&self,
		attempt: Uuid,
		window: usize,
		output: u32,
	) -> Result<Option<InferenceReservation>> {
		let mut access = self.access.write().await;
		crate::generation::budget::reserve(
			&mut access,
			self.store,
			self.run.id,
			attempt,
			window,
			output,
		)
		.await
		.map(|reservation| reservation.map(InferenceReservation::Local))
		.map_err(Into::into)
	}
}
