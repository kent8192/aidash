//! A Tenant subject's Catalog Approvals run on its exclusive Access, so each
//! `catalog.approve` decision, the approvals, their history and any agent
//! subject they create commit together or not at all.
use super::catalog_mutation::NativeMutation;
use crate::apps::identity::models::{AuthorizationBundle, AuthorizationCatalog};
use crate::apps::registry::models::Definition;
use crate::authorization::access::Access;
use aidash_application::{
	Result,
	ports::catalog::{CatalogApprovalScope, CatalogMutation},
};
use aidash_domain::{
	identity::catalog::Binding,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::orm::Model;
use serde_json::json;
use std::collections::BTreeSet;

pub(crate) struct TenantCatalog<'a> {
	pub(crate) access: &'a mut Access,
	/// The Node the approvals apply to.
	pub(crate) node: aidash_domain::nodes::NodeAddress,
}

#[async_trait]
impl CatalogMutation for TenantCatalog<'_> {
	async fn registered(&mut self, reference: &EntityRef) -> Result<bool> {
		NativeMutation(&mut self.access.tx)
			.registered(reference)
			.await
	}
	async fn compare_and_set(
		&mut self,
		tenant: &str,
		node: &aidash_domain::nodes::NodeName,
		reference: &EntityRef,
		expected_revision: i64,
		enabled: bool,
	) -> Result<Option<Binding>> {
		NativeMutation(&mut self.access.tx)
			.compare_and_set(tenant, node, reference, expected_revision, enabled)
			.await
	}
	async fn history(&mut self, binding: &Binding, actor: &str) -> Result<()> {
		NativeMutation(&mut self.access.tx)
			.history(binding, actor)
			.await
	}
}

/// Any version in the Tenant's Registry, approved or not.
pub(crate) async fn definition(
	tx: &mut crate::database::native::Transaction,
	reference: &EntityRef,
) -> Result<Entry> {
	Ok(Definition::read_in(&mut **tx, &reference.id, &reference.version).await?)
}

/// Whether this Node of the Tenant currently approves this exact version.
pub(crate) async fn approved(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	node: &aidash_domain::nodes::NodeName,
	reference: &EntityRef,
) -> Result<bool> {
	Ok(!AuthorizationCatalog::objects()
		.filter(AuthorizationCatalog::field_tenant().eq(tenant.to_owned()))
		.filter(AuthorizationCatalog::field_node().eq(node.as_str().to_owned()))
		.filter(AuthorizationCatalog::field_entry_key().eq(reference.id.clone()))
		.filter(AuthorizationCatalog::field_entry_version().eq(reference.version.clone()))
		.filter(AuthorizationCatalog::field_enabled().eq(true))
		.all_with_executor(&mut **tx)
		.await
		.map_err(FrameworkError::from)
		.map_err(crate::Error::from)?
		.is_empty())
}

#[async_trait]
impl CatalogApprovalScope for TenantCatalog<'_> {
	fn tenant(&self) -> &str {
		&self.access.identity.tenant
	}
	fn node(&self) -> &aidash_domain::nodes::NodeAddress {
		&self.node
	}
	fn actor(&self) -> &str {
		&self.access.identity.subject
	}
	async fn definition(&mut self, reference: &EntityRef) -> Result<Entry> {
		definition(&mut self.access.tx, reference).await
	}
	async fn approved(&mut self, reference: &EntityRef) -> Result<bool> {
		let tenant = self.access.identity.tenant.clone();
		approved(&mut self.access.tx, &tenant, &self.node.node, reference).await
	}
	async fn require(&mut self, entry: &Entry, action: &str) -> Result<()> {
		let resource = self.access.resource(
			"registry",
			format!("{}/{}@{}", entry.kind, entry.id, entry.version),
			aidash_application::authorization::catalog::attributes(entry),
		);
		self.access
			.require(&resource, action)
			.await
			.map_err(Into::into)
	}
	async fn require_subject(
		&mut self,
		subject: &str,
		action: &str,
		groups: &BTreeSet<String>,
	) -> Result<()> {
		let resource = self
			.access
			.resource("subject", subject, json!({"groups": groups}));
		self.access
			.require(&resource, action)
			.await
			.map_err(Into::into)
	}
	fn policy(&mut self) -> &mut aidash_domain::policy::PolicyBundle {
		&mut self.access.snapshot.bundle
	}
	async fn save_policy(&mut self) -> Result<()> {
		let tenant = self.access.identity.tenant.clone();
		let subject = self.access.identity.subject.clone();
		let revision = AuthorizationBundle::replace(
			&mut **self.access.tx,
			&tenant,
			self.access.snapshot.revision,
			serde_json::to_value(&self.access.snapshot.bundle)?,
			&subject,
			None,
		)
		.await?;
		self.access.snapshot.revision = revision;
		Ok(())
	}
}

/// Agent versions approved through a Marketplace activation become subjects of
/// the Tenant's policy too, in the default groups (ADR 0062).
pub(crate) async fn admit_agents(
	access: &mut Access,
	node: &aidash_domain::nodes::NodeAddress,
	entries: &[Entry],
) -> Result<()> {
	use aidash_domain::identity::tenant_administration as rules;
	let mut changed = false;
	for entry in entries.iter().filter(|entry| entry.kind == "agent") {
		let subject = aidash_domain::qualified_agent(node, &entry.id, &entry.version);
		if access.snapshot.bundle.subjects.contains_key(&subject) {
			continue;
		}
		let groups = rules::default_agent_groups(&access.snapshot.bundle);
		rules::add_agent_subject(&mut access.snapshot.bundle, &subject, &node.node, &groups)?;
		changed = true;
	}
	if changed {
		TenantCatalog {
			access,
			node: node.clone(),
		}
		.save_policy()
		.await?;
	}
	Ok(())
}
