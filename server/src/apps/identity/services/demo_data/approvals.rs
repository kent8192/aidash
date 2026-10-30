//! Step 4 of a Demo data load: Catalog Approvals on each Tenant's default Node.
use super::input::StructureApproval;
use crate::apps::identity::models::{AuthorizationBundle, AuthorizationCatalog};
use crate::apps::identity::repositories::catalog_approvals::{approved, definition};
use crate::apps::identity::repositories::catalog_mutation::NativeMutation;
use crate::apps::identity::serializers::contracts::Snapshot;
use crate::database::native::{Pool, Transaction};
use crate::{Error, Result};
use aidash_application::authorization::catalog::approvals::{self, Approval};
use aidash_application::ports::catalog::{CatalogApprovalScope, CatalogMutation};
use aidash_application::tenancy::TenancyScope;
use aidash_domain::{
	identity::catalog::Binding,
	nodes::{NodeAddress, NodeName},
	policy::PolicyBundle,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::orm::Model;
use std::collections::BTreeSet;

/// The history label of the approving actor, and of the policy revision that
/// adds an approved agent's subject.
const ACTOR: &str = "demo-data";

/// Approves every version in `wanted` that the Tenant does not approve yet, in
/// one transaction so that `approve` sees the whole dependency closure. A
/// version the Tenant has ever approved is left alone, enabled or not.
pub(super) async fn approve(
	pool: &Pool,
	node: &NodeAddress,
	key: &str,
	tenant: &str,
	wanted: &[StructureApproval],
) -> Result<()> {
	if wanted.is_empty() {
		return Ok(());
	}
	pool.scoped(&TenancyScope::tenant(tenant)?, async |tx| {
		let snapshot = AuthorizationBundle::lock_snapshot(&mut **tx, tenant, true).await?;
		let mut pending = Vec::new();
		for want in wanted {
			let entry = EntityRef {
				id: want.entry.id.clone(),
				version: want.entry.version.clone(),
			};
			let existing = AuthorizationCatalog::objects()
				.filter(AuthorizationCatalog::field_tenant().eq(tenant.to_owned()))
				.filter(AuthorizationCatalog::field_node().eq(node.node.as_str().to_owned()))
				.filter(AuthorizationCatalog::field_entry_key().eq(entry.id.clone()))
				.filter(AuthorizationCatalog::field_entry_version().eq(entry.version.clone()))
				.all_with_executor(&mut **tx)
				.await
				.map_err(FrameworkError::from)?
				.pop();
			// Any binding means the version was approved once; one a person
			// disabled since stays disabled.
			if existing.is_some() {
				continue;
			}
			// Groups apply to a new agent subject only; an agent version that
			// already has its subject keeps the groups it has.
			let subject = aidash_domain::qualified_agent(node, &entry.id, &entry.version);
			let groups =
				(!snapshot.bundle.subjects.contains_key(&subject)).then(|| want.groups.clone());
			pending.push(Approval {
				entry,
				enabled: true,
				expected_revision: 0,
				groups,
				restriction: None,
			});
		}
		if pending.is_empty() {
			return Ok(());
		}
		let mut scope = DemoCatalog {
			tx,
			tenant,
			node,
			snapshot,
		};
		approvals::approve(&mut scope, &pending)
			.await
			.map_err(|error| Error::Invalid(format!("tenant {key}: catalog approvals: {error}")))?;
		Ok(())
	})
	.await
}

/// The Catalog Approval scope of the `demo-data` command: one Tenant's
/// transaction with its policy locked, and no subject acting. `approve` keeps
/// every rule that does not depend on a subject: a version must be registered,
/// its dependencies must be approved first, and an approved agent version
/// becomes a subject of the policy (ADR 0062).
struct DemoCatalog<'a> {
	tx: &'a mut Transaction,
	tenant: &'a str,
	node: &'a NodeAddress,
	snapshot: Snapshot,
}

#[async_trait]
impl CatalogMutation for DemoCatalog<'_> {
	async fn registered(&mut self, reference: &EntityRef) -> aidash_application::Result<bool> {
		NativeMutation(self.tx).registered(reference).await
	}
	async fn compare_and_set(
		&mut self,
		tenant: &str,
		node: &NodeName,
		reference: &EntityRef,
		expected_revision: i64,
		enabled: bool,
		restriction: aidash_domain::identity::catalog::RestrictionChange,
	) -> aidash_application::Result<Option<Binding>> {
		NativeMutation(self.tx)
			.compare_and_set(
				tenant,
				node,
				reference,
				expected_revision,
				enabled,
				restriction,
			)
			.await
	}
	async fn history(&mut self, binding: &Binding, actor: &str) -> aidash_application::Result<()> {
		NativeMutation(self.tx).history(binding, actor).await
	}
}

#[async_trait]
impl CatalogApprovalScope for DemoCatalog<'_> {
	fn tenant(&self) -> &str {
		self.tenant
	}
	fn node(&self) -> &NodeAddress {
		self.node
	}
	fn actor(&self) -> &str {
		ACTOR
	}
	async fn definition(&mut self, reference: &EntityRef) -> aidash_application::Result<Entry> {
		definition(self.tx, reference).await
	}
	async fn approved(&mut self, reference: &EntityRef) -> aidash_application::Result<bool> {
		approved(self.tx, self.tenant, &self.node.node, reference).await
	}
	// The command acts with Deployment authority, as a Deployment
	// Administrator's `set_catalog` does, and the setting that enables it is
	// the guard. No subject exists to decide against.
	async fn require(&mut self, _: &Entry, _: &str) -> aidash_application::Result<()> {
		Ok(())
	}
	async fn require_subject(
		&mut self,
		_: &str,
		_: &str,
		_: &BTreeSet<String>,
	) -> aidash_application::Result<()> {
		Ok(())
	}
	fn policy(&mut self) -> &mut PolicyBundle {
		&mut self.snapshot.bundle
	}
	async fn save_policy(&mut self) -> aidash_application::Result<()> {
		self.snapshot.revision = AuthorizationBundle::replace(
			&mut **self.tx,
			self.tenant,
			self.snapshot.revision,
			serde_json::to_value(&self.snapshot.bundle)?,
			ACTOR,
			None,
		)
		.await?;
		Ok(())
	}
}
