//! PostgreSQL implementations of execution ports share the HTTP/worker Store.
use aidash_application::{Result, ports::ExecutionRecoveryStore};
use aidash_domain::{Run, semantic::Failure};
use async_trait::async_trait;
use uuid::Uuid;

pub struct RecoveryRepository {
	pub store: crate::store::Store,
}
#[async_trait]
impl ExecutionRecoveryStore for RecoveryRepository {
	async fn leased_run(&self, token: Uuid) -> Result<Option<Run>> {
		// Losing the lease while a step fails belongs to its new owner.
		let id = match super::services::runtime::run_id(&self.store, token).await {
			Ok(id) => id,
			Err(_) => return Ok(None),
		};
		self.store.run(id).await.map(Some).map_err(Into::into)
	}
	async fn save(&self, run: &Run, token: Uuid, event: &str) -> Result<()> {
		self.store
			.save_run(run, token, event)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn pause(&self, run: &Run, token: Uuid, reason: &str, event: &str) -> Result<()> {
		self.store
			.pause_for_execution(run, token, reason, event)
			.await
			.map_err(Into::into)
	}
	async fn pause_semantic(&self, run: &Run, token: Uuid, reason: Failure) -> Result<()> {
		self.store
			.pause_for_semantic_execution(run, token, reason)
			.await
			.map_err(Into::into)
	}
}

pub(crate) mod agent;

pub(crate) mod events;

pub mod tools;

pub mod store;

pub(crate) mod capabilities;

pub(crate) mod withdrawal;

pub(crate) mod operations;

pub(crate) mod outbound;

pub(crate) mod capability_records;
pub(crate) mod references;

pub(crate) mod approvals;

pub(crate) mod reclamation;

pub(crate) mod capability_areas;
pub(crate) mod sessions;
