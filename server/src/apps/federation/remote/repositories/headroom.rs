//! Registered definitions and private contents retain their native read authority.
use crate::federation::Federation;
use aidash_application::{
	Result, ports::execution::headroom::Definitions, registry::DefinitionValidation,
};
use aidash_domain::{RunMetadata, registry::Entry};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

pub(crate) struct Context<'a>(pub(crate) &'a Federation);

#[async_trait]
impl Definitions for Context<'_> {
	fn node(&self) -> &str {
		&self.0.config.node_id
	}
	async fn snapshot(
		&self,
		run: &RunMetadata,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
		self.0
			.store
			.run(&self.0.store.run_tenancy(run.id).await?, run.id)
			.await?
			.context
			.binding_snapshot
			.map(|snapshot| *snapshot)
			.ok_or_else(|| aidash_application::Error::Invalid("Run has no Binding snapshot".into()))
	}
	async fn definition(&self, run: &RunMetadata, id: &str, version: &str) -> Result<Entry> {
		let tenant = self.0.store.run_tenant(run.id).await?;
		self.0
			.registry
			.get_for_run(&tenant, run.clone(), id, version)
			.await
			.map_err(Into::into)
	}
	async fn documents(&self, run: &RunMetadata, agent: &Entry) -> Result<Value> {
		let tenant = self.0.store.run_tenant(run.id).await?;
		crate::knowledge::load(&self.0.registry, &tenant, agent)
			.await
			.map_err(Into::into)
	}
	fn validation(&self) -> DefinitionValidation {
		crate::bootstrap::registry_validation_for(&self.0.store)
	}
	async fn pinned_headroom(&self, run: Uuid) -> Result<usize> {
		aidash_application::capabilities::skills::context_headroom_reserve(
			&mut crate::bootstrap::skill_headroom(&self.0.store),
			run,
		)
		.await
	}
}
