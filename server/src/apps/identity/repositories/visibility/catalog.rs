//! Native definition and binding reads retain the caller's catalog lock mode.
use super::NativeReads;
use aidash_application::{Result, ports::catalog::retained::RetainedCatalogScope};
use aidash_domain::{
	policy::Resource,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
#[async_trait]
impl RetainedCatalogScope for NativeReads<'_> {
	fn inherited(&self) -> bool {
		self.access.inherited_lease()
	}
	fn approved(&self, reference: &EntityRef) -> bool {
		self.access.catalog_approved(reference)
	}
	fn remember(&mut self, reference: &EntityRef) {
		self.access.remember_catalog(reference);
	}
	async fn enabled(&mut self, reference: &EntityRef, lock: bool) -> Result<Option<bool>> {
		let (tx, tenant) = self.access.tenant_transaction();
		crate::apps::identity::models::AuthorizationCatalog::enabled_in(tx, tenant, reference, lock)
			.await
			.map_err(Into::into)
	}
	async fn definition(&mut self, reference: &EntityRef) -> Result<Entry> {
		crate::apps::registry::models::Definition::read_in(
			self.access.tx.as_mut(),
			&reference.id,
			&reference.version,
		)
		.await
		.map_err(Into::into)
	}
	fn resource(&self, entry: &Entry) -> Resource {
		self.access.resource(
			&entry.kind,
			&entry.id,
			aidash_application::authorization::catalog::attributes(entry),
		)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
}
