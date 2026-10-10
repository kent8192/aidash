//! Native version registration preserves provenance and historical attachments in one transaction.
use super::capability_records::domain;
use crate::apps::execution::capabilities::{
	serializers::configuration::Configure as NativeConfigure,
	services::{references, sessions},
};
use crate::{
	authorization::access::Access,
	registry::{EntityRef, Entry},
	store::Store,
};
use aidash_application::{
	Result,
	ports::capabilities::configuration::{ConfigurationScope, Limits},
};
use aidash_domain::capabilities::{configuration::Configure, records::Record};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct Scope<'a> {
	pub(crate) store: &'a Store,
	pub(crate) access: &'a mut Access,
}
impl From<NativeConfigure> for Configure {
	fn from(v: NativeConfigure) -> Self {
		Self {
			idempotency_key: v.idempotency_key,
			source_version: v.source_version,
			new_version: v.new_version,
			bindings: v.bindings,
			remove_default: v.remove_default,
		}
	}
}
#[async_trait]
impl ConfigurationScope for Scope<'_> {
	fn principal(&self) -> &str {
		&self.access.identity.subject
	}
	fn limits(&self) -> Limits {
		let l = &self.store.capabilities.0.limits;
		Limits {
			files: l.reference_files,
			bytes: l.reference_bytes,
			text_bytes: l.reference_text_bytes,
		}
	}
	async fn entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		crate::authorization::catalog::entry(self.access, reference, action)
			.await
			.map_err(Into::into)
	}
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>> {
		sessions::cached(self.access, key, digest)
			.await
			.map_err(Into::into)
	}
	async fn cache(&mut self, key: Uuid, digest: &str, result: &Value) -> Result<()> {
		sessions::cache(self.access, key, digest, result)
			.await
			.map_err(Into::into)
	}
	async fn reference(&mut self, id: Uuid) -> Result<Record> {
		references::get(self.access, id, "reference.read")
			.await
			.map(domain)
			.map_err(Into::into)
	}
	async fn bindings(
		&mut self,
		entry: &Entry,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
		let snapshot = crate::apps::registry::repositories::bindings::snapshot(
			&mut self.access.tx,
			&self.store.node_id,
			entry,
			false,
			self.store.provider_credentials.is_some(),
		)
		.await?;
		for definition in &snapshot.definitions {
			if definition.identity == snapshot.agent {
				continue;
			}
			let current = crate::authorization::catalog::entry(
				self.access,
				&definition.identity.local(),
				"registry.read",
			)
			.await?;
			if aidash_domain::registry::rules::digest(&serde_json::to_value(current)?)
				!= definition.digest
			{
				return Err(aidash_application::Error::Forbidden);
			}
		}
		Ok(snapshot)
	}
	async fn register(&mut self, entry: &Entry) -> Result<bool> {
		crate::registry::register_in(
			&mut self.access.tx,
			entry,
			&self.store.node_id,
			&crate::bootstrap::registry_validation_for(self.store),
		)
		.await
		.map_err(Into::into)
	}
	async fn provenance(&mut self, source: &EntityRef, entry: &Entry) -> Result<()> {
		crate::marketplace::propagate_provenance(
			&mut self.access.tx,
			source,
			entry,
			&self.access.identity.tenant,
		)
		.await
		.map_err(Into::into)
	}

	async fn event(&mut self, kind: &str, data: Value) -> Result<()> {
		self.store
			.event(&mut self.access.tx, None, kind, data)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
}
