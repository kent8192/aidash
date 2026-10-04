//! Scoped grant adapters retain native atomic writes and the caller's inference authority.
use crate::{
	apps::identity::services::{access::Access, catalog, peer},
	domain::{Run, RunMetadata},
	registry::EntityRef,
	store::Store,
};
use aidash_application::{
	Result,
	ports::{
		authorization::inference::InferenceScope,
		execution::cancellation::ScopedCancellationRepository,
	},
};
use aidash_domain::policy::Resource;
use async_trait::async_trait;
use uuid::Uuid;
pub(crate) struct Cancellation<'a> {
	pub(crate) store: &'a Store,
}
#[async_trait]
impl ScopedCancellationRepository for Cancellation<'_> {
	async fn remote_grant(&self, run: &RunMetadata) -> Result<bool> {
		peer::admission::run_grant(self.store, run)
			.await
			.map(|grant| grant.is_some())
			.map_err(Into::into)
	}
	async fn local_grant(&self, run: &RunMetadata) -> Result<bool> {
		aidash_application::authorization::execution::grant(
			&crate::bootstrap::execution_grants(self.store),
			run,
		)
		.await
		.map(|grant| grant.is_some())
	}
	async fn save_receiver(&self, run: &Run, token: Uuid, event: &str) -> Result<()> {
		self.store
			.save_run(run, token, event)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn cancel_local(&self, run: &Run, token: Uuid) -> Result<()> {
		self.store
			.cancel_execution(run, token)
			.await
			.map_err(Into::into)
	}
}
pub(crate) struct InferenceApproval<'a> {
	pub(crate) access: &'a mut Access,
	pub(crate) run: &'a Run,
	pub(crate) remote: bool,
}
#[async_trait]
impl InferenceScope for InferenceApproval<'_> {
	fn remote(&self) -> bool {
		self.remote
	}
	fn node_id(&self) -> Option<&str> {
		self.access.execution_node()
	}
	async fn context_authority(&mut self, run: &RunMetadata) -> Result<()> {
		debug_assert_eq!(run.id, self.run.id);
		crate::capabilities::sessions::context_authority(self.access, self.run)
			.await
			.map_err(Into::into)
	}
	async fn require_live(
		&mut self,
		node: &str,
		run: &RunMetadata,
		agent: &EntityRef,
	) -> Result<()> {
		crate::generation::provision::require_live(self.access, node, run.task_id, agent)
			.await
			.map_err(Into::into)
	}
	async fn catalog(&mut self, reference: &EntityRef, action: &str) -> Result<()> {
		catalog::entry(self.access, reference, action)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn memory_resource(&mut self, run: &RunMetadata) -> Result<Resource> {
		debug_assert_eq!(run.id, self.run.id);
		self.access
			.memory_resource(self.run)
			.await
			.map_err(Into::into)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
}
