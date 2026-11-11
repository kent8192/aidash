//! Native version registration preserves provenance and historical attachments in one transaction.
use super::capability_records::domain;
use crate::apps::execution::capabilities::{
	serializers::configuration::Configure as NativeConfigure, services::sessions,
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
	/// Binding an approved Source asks only that its files can still be used:
	/// the person registering the Source was asked whether they may read them.
	async fn reference(&mut self, id: Uuid) -> Result<Record> {
		crate::apps::execution::capabilities::services::records::get_many(
			self.access,
			&[id],
			"reference",
		)
		.await
		.map_err(aidash_application::Error::from)?
		.pop()
		.filter(|record| !matches!(record.state.as_str(), "revoked" | "expired"))
		.map(domain)
		.ok_or_else(|| aidash_application::Error::NotFound("reference unavailable".into()))
	}
	async fn bindings(
		&mut self,
		entry: &Entry,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
		let snapshot = crate::apps::registry::repositories::bindings::snapshot(
			&mut self.access.tx,
			self.store.deployment.as_str(),
			entry,
			false,
			self.store.provider_credentials.is_some(),
		)
		.await?;
		for definition in &snapshot.definitions {
			if definition.identity == snapshot.agent
				|| aidash_domain::registry::bindings::defaults::is_implicit_compaction(
					&snapshot,
					&definition.identity,
				) {
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
		let resource = self.access.resource(
			"registry",
			format!("{}/{}", entry.kind, entry.id),
			serde_json::json!({"kind": entry.kind, "id": entry.id, "version": entry.version}),
		);
		self.access.require(&resource, "registry.create").await?;
		crate::registry::register_in(
			&mut self.access.tx,
			entry,
			self.store.deployment.as_str(),
			&crate::bootstrap::registry_validation_for(self.store),
		)
		.await
		.map_err(Into::into)
	}
	async fn carry(&mut self, entry: &Entry, inserted: bool) -> Result<(bool, Vec<Value>)> {
		let tenant = self.access.identity.tenant.clone();
		let node = self.access.catalog_address().await?;
		let results = if inserted {
			let stored = crate::apps::identity::repositories::catalog_approvals::definition(
				&mut self.access.tx,
				&EntityRef {
					id: entry.id.clone(),
					version: entry.version.clone(),
				},
			)
			.await?;
			crate::apps::identity::repositories::catalog_carry::carry_best_effort(
				&mut crate::apps::identity::repositories::catalog_carry::TenantCarry {
					tx: &mut self.access.tx,
					snapshot: &mut self.access.snapshot,
					tenant: &tenant,
					deployment: &self.store.deployment,
				},
				&stored,
			)
			.await?
		} else {
			Vec::new()
		};
		let approved = crate::apps::identity::repositories::catalog_approvals::approved(
			&mut self.access.tx,
			&tenant,
			&node.node,
			&EntityRef {
				id: entry.id.clone(),
				version: entry.version.clone(),
			},
		)
		.await?;
		let results = results
			.into_iter()
			.map(|result| {
				serde_json::json!({
					"node": result.node.node.as_str(),
					"source_version": result.source_version,
					"changed": result.changed,
					"unapproved_dependencies": result.unapproved_dependencies,
					"carried": result.carried,
				})
			})
			.collect();
		Ok((!approved, results))
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
