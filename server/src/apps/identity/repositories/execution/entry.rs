//! The shared worker entry adapter owns exactly the existing native Access lease.
use crate::{
	apps::identity::services::{
		access::Access,
		execution::{access_for_run, authorize_guard},
		peer,
	},
	domain::{Run, RunMetadata},
	federation::Federation,
	registry::AgentConfig,
};
use aidash_application::{Result, ports::authorization::worker_entry::WorkerEntryRepository};
use async_trait::async_trait;
pub(crate) struct Entries<'a> {
	pub(crate) federation: &'a Federation,
}
#[async_trait]
impl WorkerEntryRepository<Access> for Entries<'_> {
	async fn receiver_lease(&self, run: &RunMetadata) -> Result<Option<(Access, AgentConfig)>> {
		peer::admission::worker_lease(self.federation, run)
			.await
			.map_err(Into::into)
	}
	async fn local_lease(&self, run: &RunMetadata, durable_audit: bool) -> Result<Option<Access>> {
		let mut lease = access_for_run(&self.federation.store, run, durable_audit).await?;
		if let Some(access) = &mut lease {
			access.wire = Some(crate::bootstrap::peer_transport(self.federation));
		}
		Ok(lease)
	}
	async fn authorize(
		&self,
		scope: &mut Access,
		run: &RunMetadata,
		read_context: bool,
	) -> Result<AgentConfig> {
		authorize_guard(self.federation, run, scope, read_context)
			.await
			.map_err(Into::into)
	}
	async fn scratch_space(&self, scope: &mut Access, run: &Run) -> Result<uuid::Uuid> {
		let mut services = crate::apps::workspaces::repositories::scratch::NativeScratchServices {
			policy: &self.federation.config.scratch_policy,
		};
		let mut adapter = crate::apps::workspaces::repositories::scratch::ScratchAdapter {
			access: scope,
			services: &mut services,
			person_override: None,
		};
		aidash_application::scratch::scratch_space_for_run(&mut adapter, run, chrono::Utc::now())
			.await
	}
	async fn initialize(&self, scope: &Access, run: &Run, agent: &AgentConfig) -> Result<()> {
		let mut initialization = Access::under_lease(scope).await?;
		let result = crate::capabilities::sessions::initialize(
			&self.federation.store,
			&mut initialization,
			run,
			agent,
		)
		.await;
		initialization.finish(result).await.map_err(Into::into)
	}
}
