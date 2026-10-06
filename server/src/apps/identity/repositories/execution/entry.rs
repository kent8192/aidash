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
		access_for_run(&self.federation.store, run, durable_audit)
			.await
			.map_err(Into::into)
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
