//! Permission inspection keeps effective definitions, catalog rows and policy in one transaction.
use crate::Result;
use aidash_domain::{
	identity::Principal,
	policy::{Decision, Evaluation},
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait PermissionScope: Send {
	async fn bindings(
		&mut self,
		entry: &aidash_domain::registry::Entry,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot>;
	async fn require_inspection(&mut self, entry: &EntityRef) -> Result<()>;
	async fn effective(&mut self, entry: &EntityRef) -> Result<Entry>;
	/// Retain the existing shared catalog row lock.
	async fn catalog_enabled(&mut self, tenant: &str, entry: &EntityRef) -> Result<Option<bool>>;
	async fn evaluate(&mut self, tenant: &str, input: &Evaluation) -> Result<Decision>;
	/// Read ownership for the selected tenant under the existing shared lock.
	async fn workspace_owner(&mut self, workspace: Uuid, tenant: &str) -> Result<Option<String>>;
	async fn commit(self: Box<Self>) -> Result<()>;
}
#[async_trait]
pub trait PermissionRepository: Send + Sync {
	fn principal(&self) -> Principal;
	fn node_id(&self) -> &str;
	async fn begin(&self) -> Result<Box<dyn PermissionScope + '_>>;
}
