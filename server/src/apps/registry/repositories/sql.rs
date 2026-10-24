//! Registry reads and immutable inserts on a caller-owned native transaction,
//! within the Registry of its Tenant scope.
use crate::apps::registry::models::{Definition, transaction_records};
use aidash_application::{
	Result,
	ports::registry::{DefinitionLookup, DefinitionWriter},
};
use aidash_domain::registry::Entry;
use async_trait::async_trait;
pub(crate) struct SqlScope<'a>(pub(crate) &'a mut crate::database::native::Transaction);
#[async_trait]
impl DefinitionLookup for SqlScope<'_> {
	async fn definition(&mut self, id: &str, version: &str) -> Result<Entry> {
		Ok(Definition::read_in(&mut **self.0, id, version).await?)
	}
	async fn agent_plugin(
		&mut self,
		digest: &str,
	) -> Result<Option<aidash_application::ports::bindings::StoredAgentPlugin>> {
		super::agent_plugins::get(&mut **self.0, digest)
			.await
			.map_err(Into::into)
	}
	async fn binding_installation(
		&mut self,
		projection: &aidash_domain::registry::Projection,
	) -> Result<()> {
		super::bindings::installation_native(self.0, projection)
			.await
			.map_err(Into::into)
	}
	async fn executor_kind(&mut self, id: &str, version: &str) -> Result<Option<String>> {
		match Definition::read_in(&mut **self.0, id, version).await {
			Ok(entry) => Ok(Some(entry.kind)),
			Err(crate::Error::NotFound(_)) => Ok(None),
			Err(error) => Err(error.into()),
		}
	}
	async fn tenant_secret_hosts(&mut self, name: &str) -> Result<Option<Vec<String>>> {
		crate::apps::identity::repositories::tenant_secrets::active_hosts(&mut **self.0, name).await
	}
}
#[async_trait]
impl DefinitionWriter for SqlScope<'_> {
	async fn insert_definition(&mut self, entry: &Entry) -> Result<bool> {
		transaction_records::insert_definition(&mut **self.0, entry)
			.await
			.map_err(Into::into)
	}
}
