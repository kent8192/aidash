//! The Tenant's Registry as its subjects administer it: every version, with
//! its approval state and history, on the subject's Access transaction.
use crate::apps::identity::models::{AuthorizationCatalog, AuthorizationCatalogHistory};
use crate::apps::registry::models::Definition;
use crate::apps::registry::serializers::contracts::{
	AgentSubject, ApprovalRecord, Entry, RegistryVersion, VersionApproval,
};
use crate::authorization::access::Access;
use crate::{Error, Result};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::orm::Model;

const PAGE: usize = 100;

pub(crate) async fn page(
	access: &mut Access,
	node: &str,
	offset: u64,
) -> Result<Vec<RegistryVersion>> {
	let resource = access.resource(
		"registry",
		aidash_domain::identity::tenant_administration::COLLECTION,
		serde_json::json!({}),
	);
	access.require(&resource, "registry.read").await?;
	let tenant = access.identity.tenant.clone();
	let rows = Definition::objects()
		.filter(Definition::field_tenant().eq(tenant.clone()))
		.order_by(&["id", "version"])
		.offset(usize::try_from(offset).map_err(|_| Error::Invalid("offset".into()))?)
		.limit(PAGE)
		.all_with_executor(&mut **access.tx)
		.await
		.map_err(FrameworkError::from)?;
	let mut versions = Vec::with_capacity(rows.len());
	for row in rows {
		let entry: Entry = serde_json::from_value(row.metadata.0)?;
		let approval = AuthorizationCatalog::objects()
			.filter(AuthorizationCatalog::field_tenant().eq(tenant.clone()))
			.filter(AuthorizationCatalog::field_entry_key().eq(entry.id.clone()))
			.filter(AuthorizationCatalog::field_entry_version().eq(entry.version.clone()))
			.all_with_executor(&mut **access.tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.map(|row| VersionApproval {
				enabled: row.enabled,
				revision: row.revision,
			});
		let history = AuthorizationCatalogHistory::objects()
			.filter(AuthorizationCatalogHistory::field_tenant().eq(tenant.clone()))
			.filter(AuthorizationCatalogHistory::field_entry_key().eq(entry.id.clone()))
			.filter(AuthorizationCatalogHistory::field_entry_version().eq(entry.version.clone()))
			.order_by(&["revision"])
			.all_with_executor(&mut **access.tx)
			.await
			.map_err(FrameworkError::from)?
			.into_iter()
			.map(|row| ApprovalRecord {
				revision: row.revision,
				enabled: row.enabled,
				actor: row.actor,
				created_at: row.created_at,
			})
			.collect();
		let subject = (entry.kind == "agent")
			.then(|| aidash_domain::qualified_agent(node, &entry.id, &entry.version))
			.and_then(|id| {
				access
					.snapshot
					.bundle
					.subjects
					.get(&id)
					.map(|subject| AgentSubject {
						groups: subject.groups.clone(),
						enabled: subject.enabled,
						id,
					})
			});
		versions.push(RegistryVersion {
			needs_approval: !aidash_application::registry::system::is_builtin(&entry),
			entry,
			approval,
			history,
			subject,
		});
	}
	Ok(versions)
}
