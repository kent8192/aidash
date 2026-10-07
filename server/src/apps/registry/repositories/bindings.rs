//! Exact installed revisions are admitted under the caller's transaction.
use crate::{
	Error, Result,
	apps::{
		identity::models::AuthorizationCatalog,
		marketplace::models::{InstalledPackage, InstalledRevision},
	},
};
use aidash_domain::{
	marketplace::{Installation, Revision, definitions::key},
	registry::Projection,
};
use reinhardt::db::orm::{Model, OrmExecutor};

pub(crate) async fn snapshot(
	tx: &mut crate::database::native::Transaction,
	node: &str,
	agent: &aidash_domain::registry::Entry,
	remote: bool,
) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
	Ok(aidash_application::registry::bindings::resolve(
		&mut aidash_application::registry::bindings::catalog::LookupCatalog {
			definitions: &mut super::sql::SqlScope(tx),
			node,
		},
		&crate::bootstrap::registry_validation(),
		aidash_domain::registry::bindings::QualifiedRef {
			registry_node: node.into(),
			id: agent.id.clone(),
			version: agent.version.clone(),
		},
		agent,
		remote,
	)
	.await?)
}

/// Registration previews check current pointers without a row lease; execution
/// admission uses installation_executor under its caller-owned transaction.
pub(crate) async fn installation<E: OrmExecutor>(
	db: &mut E,
	projection: &Projection,
) -> Result<()> {
	if projection.contract != 1 {
		return Err(Error::Forbidden);
	}
	let row = InstalledPackage::objects()
		.filter(InstalledPackage::field_key().eq(projection.installation.clone()))
		.all_with_db(db)
		.await?
		.pop()
		.ok_or(Error::Forbidden)?;
	let installed: Installation = serde_json::from_value(row.document.0)?;
	if installed.tenant != projection.tenant
		|| installed.active_revision != Some(projection.revision)
	{
		return Err(Error::Forbidden);
	}
	let row = InstalledRevision::objects()
		.filter(
			InstalledRevision::field_key()
				.eq(key(&(&projection.installation, projection.revision))),
		)
		.all_with_db(db)
		.await?
		.pop()
		.ok_or(Error::Forbidden)?;
	let revision: Revision = serde_json::from_value(row.document.0)?;
	let approved = AuthorizationCatalog::objects()
		.filter(AuthorizationCatalog::field_tenant().eq(projection.tenant.clone()))
		.filter(AuthorizationCatalog::field_entry_key().eq(revision.entry.id))
		.filter(AuthorizationCatalog::field_entry_version().eq(revision.entry.version))
		.filter(AuthorizationCatalog::field_enabled().eq(true))
		.all_with_db(db)
		.await?;
	if approved.is_empty() {
		return Err(Error::Forbidden);
	}
	Ok(())
}

/// Native transaction adapters reuse Marketplace's retained-revision reads.
pub(crate) async fn installation_native(
	tx: &mut crate::database::native::Transaction,
	projection: &Projection,
) -> Result<()> {
	installation_executor(&mut **tx, projection).await
}

/// Framework transactions use the same rows and shared distribution fence.
pub(crate) async fn installation_executor(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	projection: &Projection,
) -> Result<()> {
	use reinhardt::{
		core::exception::Error as FrameworkError,
		query::{Expr, PostgresQueryBuilder, Query, QueryStatementBuilder},
	};
	let (sql, _) = Query::select()
		.expr(Expr::cust("pg_advisory_xact_lock_shared(74003201)"))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, vec![]).await?;
	if projection.contract != 1 {
		return Err(Error::Forbidden);
	}
	let row = InstalledPackage::objects()
		.filter(InstalledPackage::field_key().eq(projection.installation.clone()))
		.select_for_update()
		.all_with_executor(tx)
		.await
		.map_err(FrameworkError::from)?
		.pop()
		.ok_or(Error::Forbidden)?;
	let installed: Installation = serde_json::from_value(row.document.0)?;
	if installed.tenant != projection.tenant
		|| installed.active_revision != Some(projection.revision)
	{
		return Err(Error::Forbidden);
	}
	let row = InstalledRevision::objects()
		.filter(
			InstalledRevision::field_key()
				.eq(key(&(&projection.installation, projection.revision))),
		)
		.all_with_executor(tx)
		.await
		.map_err(FrameworkError::from)?
		.pop()
		.ok_or(Error::Forbidden)?;
	let revision: Revision = serde_json::from_value(row.document.0)?;
	let approved = AuthorizationCatalog::objects()
		.filter(AuthorizationCatalog::field_tenant().eq(projection.tenant.clone()))
		.filter(AuthorizationCatalog::field_entry_key().eq(revision.entry.id))
		.filter(AuthorizationCatalog::field_entry_version().eq(revision.entry.version))
		.filter(AuthorizationCatalog::field_enabled().eq(true))
		.select_for_update()
		.all_with_executor(tx)
		.await
		.map_err(FrameworkError::from)?;
	if approved.is_empty() {
		return Err(Error::Forbidden);
	}
	Ok(())
}

pub(crate) async fn authorized(
	access: &mut crate::authorization::access::Access,
	entry: &aidash_domain::registry::Entry,
	remote: bool,
) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
	let node = access.node_id.clone();
	let snapshot = snapshot(&mut access.tx, &node, entry, remote).await?;
	for definition in &snapshot.definitions {
		if definition.identity.registry_node != node {
			return Err(Error::Forbidden);
		}
		let current = crate::authorization::catalog::entry(
			access,
			&definition.identity.local(),
			"registry.read",
		)
		.await?;
		if aidash_domain::registry::rules::digest(&serde_json::to_value(current)?)
			!= definition.digest
		{
			return Err(Error::Conflict(
				"Binding definition changed during admission".into(),
			));
		}
	}
	Ok(snapshot)
}

/// Resolve a preview/template closure in an existing framework transaction.
pub(crate) async fn preview(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	node: &str,
	entry: &aidash_domain::registry::Entry,
) -> aidash_application::Result<aidash_domain::registry::bindings::BindingSnapshot> {
	aidash_application::registry::bindings::resolve(
		&mut aidash_application::registry::bindings::catalog::LookupCatalog {
			definitions: &mut super::NativeScope(tx),
			node,
		},
		&crate::bootstrap::registry_validation(),
		aidash_domain::registry::bindings::QualifiedRef {
			registry_node: node.into(),
			id: entry.id.clone(),
			version: entry.version.clone(),
		},
		entry,
		false,
	)
	.await
}

/// A receipt can use only the exact operation of an already admitted Run.
pub(crate) async fn require_operation(
	access: &mut crate::authorization::access::Access,
	run: uuid::Uuid,
	operation: &str,
) -> aidash_application::Result<()> {
	let run: aidash_domain::Run =
		crate::apps::execution::models::Run::read_in(&mut **access.tx, run, None)
			.await?
			.ok_or(aidash_application::Error::Forbidden)?;
	let snapshot =
		run.context.binding_snapshot.as_ref().ok_or_else(|| {
			aidash_application::Error::Invalid("Run has no Binding snapshot".into())
		})?;
	snapshot.validate()?;
	if snapshot.agent.registry_node != access.node_id {
		return Err(aidash_application::Error::Forbidden);
	}
	let binding = snapshot.operation(operation)?;
	let current =
		crate::authorization::catalog::entry(access, &binding.identity.local(), "registry.read")
			.await?;
	if aidash_domain::registry::rules::digest(&serde_json::to_value(&current)?) != binding.digest {
		return Err(aidash_application::Error::Forbidden);
	}
	crate::marketplace::check_pinned(access, &current).await?;
	let resource = access.resource(
		"tool",
		binding.identity.resource_id(),
		serde_json::json!({"version":binding.identity.version}),
	);
	access.require(&resource, "tool.invoke").await?;
	Ok(())
}

/// System visibility verifies the exact Node-owned bytes and grants no resource permission.
pub(crate) async fn system_definition(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	node: &str,
	reference: &aidash_domain::registry::EntityRef,
) -> crate::Result<aidash_domain::registry::Entry> {
	let expected = aidash_application::registry::system::entries(
		&crate::bootstrap::registry_validation(),
		node,
	)?
	.into_iter()
	.find(|entry| entry.id == reference.id && entry.version == reference.version)
	.ok_or(crate::Error::Forbidden)?;
	let saved = crate::apps::registry::models::transaction_records::definition(
		tx,
		&reference.id,
		&reference.version,
	)
	.await?;
	let entry: aidash_domain::registry::Entry = serde_json::from_value(saved.metadata.0)?;
	if entry != expected {
		return Err(crate::Error::Conflict(
			"system declaration bytes changed".into(),
		));
	}
	Ok(entry)
}
