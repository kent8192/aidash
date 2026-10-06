//! Current catalog authority, inherited approval fences and exact retained definitions.
use crate::Result;
use aidash_domain::{
	policy::Resource,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use serde_json::Value;
#[async_trait]
pub trait CatalogScope: Send {
	fn tenant(&self) -> &str;
	fn inherited_lease(&self) -> bool;
	fn approved(&self, reference: &EntityRef) -> bool;
	fn remember(&mut self, reference: &EntityRef);
	async fn distribution_lock(&mut self) -> Result<()>;
	async fn document(&mut self, reference: &EntityRef) -> Result<Option<Value>>;
	async fn documents(&mut self) -> Result<Vec<Value>>;
	fn resource(&self, entry: &Entry) -> Resource;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	/// Current Marketplace activation and dependency authority borrow this lease.
	async fn active(&mut self, entry: &Entry) -> Result<bool>;
	async fn check_pinned(&mut self, entry: &Entry) -> Result<()>;
}

/// A protected mutation borrows the caller's transaction and authority locks.
/// Neither an approval update nor its history may commit independently.
#[async_trait]
pub trait CatalogMutation: Send {
	async fn registered(&mut self, reference: &EntityRef) -> Result<bool>;
	async fn compare_and_set(
		&mut self,
		tenant: &str,
		reference: &EntityRef,
		expected_revision: i64,
		enabled: bool,
	) -> Result<Option<aidash_domain::identity::catalog::Binding>>;
	async fn history(
		&mut self,
		binding: &aidash_domain::identity::catalog::Binding,
		actor: &str,
	) -> Result<()>;
}

#[async_trait]
pub trait CatalogAdministrator: CatalogMutation {
	fn principal(&self) -> &aidash_domain::identity::Principal;
	async fn load_tenant(&mut self, tenant: &str) -> Result<()>;
	async fn definition(&mut self, reference: &EntityRef) -> Result<Entry>;
}

#[async_trait]
pub trait CatalogAdministrationRead: Send {
	fn principal(&self) -> &aidash_domain::identity::Principal;
	async fn bindings(
		&mut self,
		tenant: &str,
	) -> Result<Vec<aidash_domain::identity::catalog::Binding>>;
}

pub mod retained;
