use super::{Authorization, access::Access, identity::SubjectIdentity};
use crate::{
	Result,
	registry::{EntityRef, Entry, Search},
	store::Store,
};

impl Authorization {
	pub async fn set_catalog(
		&self,
		tenant: &str,
		node: &aidash_domain::nodes::NodeName,
		entry: &EntityRef,
		expected_revision: i64,
		enabled: bool,
		actor: &str,
	) -> Result<Binding> {
		aidash_domain::identity::catalog::validate_revision(expected_revision)?;
		let mut tx = self
			.pool
			.begin(&aidash_application::tenancy::TenancyScope::tenant(tenant)?)
			.await?;
		let binding = aidash_application::authorization::catalog::set_catalog(
			&mut crate::bootstrap::catalog_administrator_scope(
				&mut tx,
				aidash_domain::identity::Principal::DeploymentAdministrator,
			),
			tenant,
			node,
			entry,
			expected_revision,
			enabled,
			actor,
		)
		.await?;
		tx.commit().await?;
		Ok(binding)
	}

	pub async fn catalog(
		&self,
		tenant: &str,
		node: &aidash_domain::nodes::NodeName,
	) -> Result<Vec<Binding>> {
		crate::apps::identity::repositories::catalog_mutation::bindings(&self.pool, tenant, node)
			.await
	}
}

pub(crate) async fn entry(
	access: &mut Access,
	reference: &EntityRef,
	action: &str,
) -> Result<Entry> {
	aidash_application::authorization::catalog::entry(
		&mut crate::bootstrap::catalog_scope(access),
		reference,
		action,
	)
	.await
	.map_err(Into::into)
}

/// [`entry`] for the call of `tool`: a policy denial is not reported as a Catalog Approval refusal.
pub(crate) async fn tool_entry(
	access: &mut Access,
	reference: &EntityRef,
	action: &str,
	tool: &str,
) -> Result<Entry> {
	aidash_application::authorization::catalog::tool_entry(
		&mut crate::bootstrap::catalog_scope(access),
		reference,
		action,
		tool,
	)
	.await
	.map_err(Into::into)
}

/// A pinned dormant Decider still needs readable immutable Registry metadata.
/// This path neither acquires Catalog Approval nor records an invocation/read
/// grant; the decision gate must separately authorize every disclosure.
pub(crate) async fn metadata(access: &mut Access, reference: &EntityRef) -> Result<Entry> {
	let entry = crate::apps::registry::models::Definition::read_in(
		&mut **access.tx,
		&reference.id,
		&reference.version,
	)
	.await?;
	if !aidash_domain::registry::bindings::defaults::is_compaction_decider(&entry)
		&& !aidash_domain::registry::bindings::sources::is_selection_decider(&entry)
	{
		return Err(crate::Error::Forbidden);
	}
	if let Some(projection) = &entry.installation
		&& (projection.contract != 1 || projection.tenant != access.identity.tenant)
	{
		return Err(crate::Error::Forbidden);
	}
	access
		.require(&resource(access, &entry), "registry.read")
		.await?;
	crate::marketplace::check_pinned(access, &entry).await?;
	if aidash_domain::registry::bindings::defaults::is_compaction_decider(&entry) {
		// A child Run admitted under this lease starts only with its own
		// compaction Decider approved (#540). The inherited-lease fence admits
		// a definition only when the parent's closure names it; this one is a
		// closure member although its approval is checked when needed. The
		// current approval is still read by the child's own start check.
		access
			.approved_catalog
			.insert((reference.id.clone(), reference.version.clone()));
	}
	Ok(entry)
}

pub(crate) fn resource(access: &Access, entry: &Entry) -> super::policy::Resource {
	if entry.kind == "tool"
		&& let Ok(descriptor) = serde_json::from_value::<
			aidash_domain::tool::providers::ToolDescriptor,
		>(entry.config.clone())
	{
		let identity = aidash_domain::registry::bindings::QualifiedRef {
			registry_deployment: descriptor.registry_deployment,
			id: entry.id.clone(),
			version: entry.version.clone(),
		};
		return access.resource(
			"tool",
			identity.resource_id(),
			aidash_application::authorization::catalog::attributes(entry),
		);
	}
	access.resource(
		&entry.kind,
		&entry.id,
		aidash_application::authorization::catalog::attributes(entry),
	)
}

pub(crate) async fn list_in(access: &mut Access, search: &Search) -> Result<Vec<Entry>> {
	aidash_application::authorization::catalog::list(
		&mut crate::bootstrap::catalog_scope(access),
		search,
	)
	.await
	.map_err(Into::into)
}

pub async fn list(
	store: &Store,
	identity: &SubjectIdentity,
	search: &Search,
) -> Result<Vec<Entry>> {
	let mut access = Access::begin(store, identity).await?;
	let result = list_in(&mut access, search).await;
	access.finish(result).await
}

pub async fn get(
	store: &Store,
	identity: &SubjectIdentity,
	reference: &EntityRef,
) -> Result<Entry> {
	let mut access = Access::begin(store, identity).await?;
	let result = aidash_application::authorization::catalog::get(
		&mut crate::bootstrap::catalog_scope(&mut access),
		reference,
	)
	.await
	.map_err(Into::into);
	access.finish(result).await
}

pub use crate::apps::identity::serializers::catalog::Binding;
