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

/// [`entry`] for a definition of an executing agent's Binding closure that
/// only the closure agents `users` use directly.
pub(crate) async fn closure_entry(
	access: &mut Access,
	reference: &EntityRef,
	action: &str,
	users: &[EntityRef],
) -> Result<Entry> {
	aidash_application::authorization::catalog::closure_entry(
		&mut crate::bootstrap::catalog_scope(access),
		reference,
		action,
		users,
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
	if !aidash_domain::registry::bindings::sources::is_selection_decider(&entry) {
		return Err(crate::Error::Forbidden);
	}
	if let Some(projection) = &entry.installation
		&& (projection.contract != 1 || projection.tenant != access.identity.tenant)
	{
		return Err(crate::Error::Forbidden);
	}
	// Decided once per transaction, like `readable`: a walk reaches the same Decider through
	// every Run that pins it.
	let read_check = access.catalog_admission(
		reference,
		aidash_application::authorization::catalog::READ_CHECK,
	);
	if access.tx.admitted(&read_check).is_none() {
		access
			.require(&resource(access, &entry), "registry.read")
			.await?;
		access.tx.admit(read_check, entry.clone());
	}
	crate::marketplace::check_pinned(access, &entry).await?;
	Ok(entry)
}

/// A definition a Run read, as its reader may see it: `registry.read` decides, and a Catalog
/// Approval or its restriction does not (#769). Unlike [`entry`], this takes no distribution
/// lock, admits no use and leaves the approved set alone ([`read_recorded`] adds to it). A
/// missing definition is `Forbidden`, as for [`entry`].
///
/// The decision is kept for the transaction under its own key, so a walk over many Runs with
/// overlapping Bindings decides each definition once. A read-check result never counts as an
/// approval; a `registry.read` admission already made by [`entry`] counts as a read check.
pub(crate) async fn readable(access: &mut Access, reference: &EntityRef) -> Result<Entry> {
	let read_check = access.catalog_admission(
		reference,
		aidash_application::authorization::catalog::READ_CHECK,
	);
	if let Some(entry) = access.tx.admitted(&read_check) {
		return Ok(entry);
	}
	if let Some(entry) = access
		.tx
		.admitted(&access.catalog_admission(reference, "registry.read"))
	{
		return Ok(entry);
	}
	let entry = crate::apps::registry::models::Definition::read_in(
		&mut **access.tx,
		&reference.id,
		&reference.version,
	)
	.await
	.map_err(|error| match error {
		crate::Error::NotFound(_) => crate::Error::Forbidden,
		other => other,
	})?;
	if let Some(projection) = &entry.installation
		&& (projection.contract != 1 || projection.tenant != access.identity.tenant)
	{
		return Err(crate::Error::Forbidden);
	}
	access
		.require(&resource(access, &entry), "registry.read")
		.await?;
	access.tx.admit(read_check, entry.clone());
	Ok(entry)
}

/// [`readable`] for a definition a Run recorded as read (its Binding, `agent_discover`, memory).
///
/// A read of the Run's own authority also puts the definition in the set a retained lease may
/// admit a delegated Run from. The read walk used to do this as a side effect of approving each
/// definition; admission still reads the current approval, so membership alone admits nothing.
/// A read under an inherited lease must not enlarge that lease, so it adds nothing there.
pub(crate) async fn read_recorded(access: &mut Access, reference: &EntityRef) -> Result<Entry> {
	let entry = readable(access, reference).await?;
	if !access.inherited_lease {
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
