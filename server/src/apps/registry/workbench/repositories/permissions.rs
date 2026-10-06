//! Native permission inspection retains current identity, effective metadata and shared catalog/workspace locks.
use crate::apps::identity::models::{AuthorizationCatalog, AuthorizationWorkspace};
use crate::{
	authorization::{Authorization, identity::Actor},
	federation::Federation,
};
use aidash_application::{
	Result,
	ports::registry::workbench::permissions::{PermissionRepository, PermissionScope},
};
use aidash_domain::{
	identity::Principal,
	policy::{Decision, Evaluation},
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use reinhardt::db::backends::TransactionExecutor;
use uuid::Uuid;
pub(crate) struct Repository<'a> {
	pub(crate) runtime: &'a Federation,
	pub(crate) actor: Actor,
}
struct Scope {
	tx: Box<dyn TransactionExecutor>,
	actor: Actor,
}
#[async_trait]
impl PermissionRepository for Repository<'_> {
	fn principal(&self) -> Principal {
		super::authority::principal(&self.actor)
	}
	fn node_id(&self) -> &str {
		&self.runtime.config.node_id
	}
	async fn begin(&self) -> Result<Box<dyn PermissionScope + '_>> {
		Ok(Box::new(Scope {
			tx: Box::new(crate::database::native::begin(&self.runtime.store.pool).await?),
			actor: self.actor.clone(),
		}))
	}
}
#[async_trait]
impl PermissionScope for Scope {
	async fn require_inspection(&mut self, entry: &EntityRef) -> Result<()> {
		aidash_application::registry::workbench::inspection::require(
			&mut crate::bootstrap::draft_authority_scope(self.tx.as_mut(), &self.actor),
			entry,
		)
		.await
	}
	async fn effective(&mut self, entry: &EntityRef) -> Result<Entry> {
		crate::apps::registry::services::admission::effective(
			self.tx.as_mut(),
			&entry.id,
			&entry.version,
		)
		.await
		.map_err(Into::into)
	}
	async fn catalog_enabled(&mut self, tenant: &str, entry: &EntityRef) -> Result<Option<bool>> {
		AuthorizationCatalog::enabled_in(self.tx.as_mut(), tenant, entry, true)
			.await
			.map_err(Into::into)
	}
	async fn evaluate(&mut self, tenant: &str, input: &Evaluation) -> Result<Decision> {
		Authorization::evaluate_native(self.tx.as_mut(), tenant, input)
			.await
			.map_err(Into::into)
	}
	async fn workspace_owner(&mut self, workspace: Uuid, tenant: &str) -> Result<Option<String>> {
		AuthorizationWorkspace::owner_in(self.tx.as_mut(), workspace, tenant, true)
			.await
			.map_err(Into::into)
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		self.tx
			.commit()
			.await
			.map_err(crate::Error::from)
			.map_err(Into::into)
	}
}
