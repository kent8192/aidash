//! Approval Review reads one Tenant's exact definitions and approvals on one
//! Node under the requester's authorization transaction.
use crate::{
	apps::identity::models::AuthorizationCatalog,
	authorization::access::Access,
	registry::{EntityRef, Entry},
};
use aidash_application::Result;
use aidash_application::ports::catalog_review::{
	ApprovalReviewScope, ApprovalSnapshot, InstalledRevision,
};
use aidash_domain::{marketplace::Installation, nodes::NodeAddress, registry::Projection};
use async_trait::async_trait;
use reinhardt::db::orm::Model;

pub(crate) struct ReviewScope<'a> {
	pub(crate) access: &'a mut Access,
	pub(crate) node: NodeAddress,
	pub(crate) validation: aidash_application::registry::DefinitionValidation,
}

#[async_trait]
impl ApprovalReviewScope for ReviewScope<'_> {
	fn tenant(&self) -> &str {
		&self.access.identity.tenant
	}
	fn node(&self) -> &NodeAddress {
		&self.node
	}
	async fn definition(&mut self, reference: &EntityRef) -> Result<Option<Entry>> {
		match crate::apps::registry::models::Definition::read_in(
			self.access.tx.native_mut(),
			&reference.id,
			&reference.version,
		)
		.await
		{
			Ok(entry) => Ok(Some(entry)),
			Err(crate::Error::NotFound(_)) => Ok(None),
			Err(error) => Err(error.into()),
		}
	}
	async fn approval(&mut self, reference: &EntityRef) -> Result<Option<ApprovalSnapshot>> {
		Ok(AuthorizationCatalog::objects()
			.filter(AuthorizationCatalog::field_tenant().eq(self.tenant().to_owned()))
			.filter(AuthorizationCatalog::field_node().eq(self.node.node.as_str().to_owned()))
			.filter(AuthorizationCatalog::field_entry_key().eq(reference.id.clone()))
			.filter(AuthorizationCatalog::field_entry_version().eq(reference.version.clone()))
			.all_with_executor(self.access.tx.native_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?
			.pop()
			.map(|row| ApprovalSnapshot {
				enabled: row.enabled,
				revision: row.revision,
				restriction: row
					.restriction
					.and_then(|value| serde_json::from_value(value.0).ok()),
			}))
	}
	async fn installed(&mut self, projection: &Projection) -> Result<Option<InstalledRevision>> {
		if projection.contract != 1 || projection.tenant != self.tenant() {
			return Ok(None);
		}
		let Some(installation): Option<Installation> =
			crate::apps::marketplace::repositories::storage::get(
				self.access.tx.native_mut(),
				"marketplace_installations",
				&projection.installation,
			)
			.await?
		else {
			return Ok(None);
		};
		if installation.tenant != self.tenant() {
			return Ok(None);
		}
		let revision = match crate::apps::marketplace::repositories::installations::revision(
			self.access.tx.native_mut(),
			&projection.installation,
			projection.revision,
		)
		.await
		{
			Ok(revision) => revision,
			Err(crate::Error::Forbidden) => return Ok(None),
			Err(error) => return Err(error.into()),
		};
		if revision.tenant != self.tenant()
			|| revision.installation != projection.installation
			|| revision.revision != projection.revision
			|| aidash_domain::marketplace::definitions::key(&revision.entry) != revision.digest
		{
			return Ok(None);
		}
		Ok(Some(InstalledRevision {
			revision: projection.revision,
			digest: revision.digest,
			activation_revision: installation.activation_revision,
			active: installation.active_revision == Some(projection.revision),
			installation,
		}))
	}
	async fn can_read(&mut self, entry: &Entry) -> Result<bool> {
		let resource = super::super::services::catalog::resource(self.access, entry);
		self.access
			.decide(&resource, "registry.read")
			.await
			.map_err(Into::into)
	}
	async fn require_read(&mut self, entry: &Entry) -> Result<()> {
		let resource = super::super::services::catalog::resource(self.access, entry);
		self.access
			.require(&resource, "registry.read")
			.await
			.map_err(Into::into)
	}
	async fn can_approve(&mut self, entry: &Entry) -> Result<bool> {
		let resource = super::super::services::catalog::resource(self.access, entry);
		self.access
			.decide(
				&resource,
				aidash_application::authorization::catalog::approvals::CATALOG_APPROVE,
			)
			.await
			.map_err(Into::into)
	}
	async fn provider_available(&mut self, entry: &Entry) -> Result<bool> {
		Ok(self.validation.provider_available(entry).is_ok())
	}
	async fn subject_groups(
		&mut self,
		subject: &str,
	) -> Result<std::collections::BTreeSet<String>> {
		Ok(self
			.access
			.snapshot
			.bundle
			.subjects
			.get(subject)
			.map(|subject| subject.groups.clone())
			.unwrap_or_default())
	}
	async fn source_slot_available(&mut self, entry: &Entry) -> Result<bool> {
		if let Some((secret, endpoint)) = aidash_domain::tenant_secrets::definition_target(entry) {
			let hosts = crate::apps::identity::repositories::tenant_secrets::active_hosts(
				self.access.tx.native_mut(),
				&secret,
			)
			.await?;
			if aidash_application::tenant_secrets::admit(&secret, &endpoint, hosts.as_deref())
				.is_err()
			{
				return Ok(false);
			}
		}
		self.validation.source_slot_available(entry)
	}
}
