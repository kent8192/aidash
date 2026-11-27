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
	configured: bool,
) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
	snapshot_mode(tx, node, agent, remote, true, configured, None).await
}
pub(crate) async fn snapshot_mode(
	tx: &mut crate::database::native::Transaction,
	node: &str,
	agent: &aidash_domain::registry::Entry,
	remote: bool,
	foreign: bool,
	configured: bool,
	task_model: Option<&aidash_domain::TaskModel>,
) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
	let mut definitions = super::sql::SqlScope(tx);
	let mut catalog = aidash_application::registry::bindings::catalog::LocalCatalog(
		aidash_application::registry::bindings::catalog::LookupCatalog {
			definitions: &mut definitions,
			node,
		},
	);
	Ok(aidash_application::registry::bindings::resolve_with_model(
		if foreign {
			&mut catalog.0
		} else {
			&mut catalog
		},
		&crate::bootstrap::registry_validation().with_provider_credentials(configured),
		aidash_domain::registry::bindings::QualifiedRef {
			registry_deployment: node.into(),
			id: agent.id.clone(),
			version: agent.version.clone(),
		},
		agent,
		remote,
		task_model,
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
		db::orm::{QueryRow, execution::convert_values},
		query::{
			Alias, ColumnRef, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query,
			QueryStatementBuilder,
		},
	};
	let (sql, _) = Query::select()
		.expr(Expr::cust("pg_advisory_xact_lock_shared(74003201)"))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, vec![]).await?;
	if projection.contract != 1 {
		return Err(Error::Forbidden);
	}
	// Validation is read-only. Shared row leases fence activation changes while
	// allowing a Host operation's inherited transaction to recheck the same
	// installed definition without waiting on its own outer authority lease.
	let (sql, values) = Query::select()
		.column(ColumnRef::Asterisk)
		.from(Alias::new(InstalledPackage::table_name()))
		.and_where(Expr::col("key").eq(Expr::value(&projection.installation)))
		.lock(LockType::Share)
		.build(PostgresQueryBuilder);
	let row: InstalledPackage = serde_json::from_value(
		QueryRow::from_backend_row(
			tx.fetch_optional(&sql, convert_values(values))
				.await?
				.ok_or(Error::Forbidden)?,
		)
		.data,
	)?;
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
	let (sql, values) = Query::select()
		.column(ColumnRef::Asterisk)
		.from(Alias::new(AuthorizationCatalog::table_name()))
		.and_where(Expr::col("tenant").eq(Expr::value(&projection.tenant)))
		.and_where(Expr::col("entry_id").eq(Expr::value(&revision.entry.id)))
		.and_where(Expr::col("entry_version").eq(Expr::value(&revision.entry.version)))
		.and_where(Expr::col("enabled").eq(true))
		.lock(LockType::Share)
		.build(PostgresQueryBuilder);
	if tx
		.fetch_optional(&sql, convert_values(values))
		.await?
		.is_none()
	{
		return Err(Error::Forbidden);
	}
	Ok(())
}

pub(crate) async fn check_task_model(
	store: &crate::store::Store,
	remote: bool,
	access: &mut crate::authorization::access::Access,
	agent: &aidash_domain::registry::Entry,
	selection: Option<&aidash_domain::TaskModel>,
	caller_subjects: &[String],
) -> Result<()> {
	use crate::apps::identity::repositories::catalog_approvals;
	use aidash_domain::TaskModelRefusal::*;
	let Some(selection) = selection else {
		return if agent.config["model"].is_null() {
			Err(TaskModelRequired.into())
		} else {
			Ok(())
		};
	};
	selection.validate().map_err(|error| TaskModelUnusable {
		reasons: vec![error.to_string()],
	})?;
	let model = match catalog_approvals::definition(&mut access.tx, &selection.model).await {
		Ok(model) if model.kind == "model" => model,
		Ok(_) | Err(aidash_application::Error::NotFound(_)) => {
			return Err(TaskModelNotRegistered.into());
		}
		Err(other) => return Err(other.into()),
	};
	let node = access.catalog_node().await?;
	if !catalog_approvals::approved(
		&mut access.tx,
		&access.identity.tenant,
		&node,
		&selection.model,
	)
	.await?
	{
		let tenant = access.identity.tenant.clone();
		let nodes =
			crate::apps::identity::repositories::nodes::list_live(&mut *access.tx, &tenant).await?;
		let previous = std::mem::replace(&mut access.subjects, caller_subjects.to_vec());
		let allowed: Result<bool> = async {
			for target in nodes {
				access.set_node(target.name);
				let resource = access.resource(
					"registry",
					format!("model/{}@{}", model.id, model.version),
					aidash_application::authorization::catalog::attributes(&model),
				);
				if !access.decide_hint(&resource, "catalog.approve").await? {
					return Ok(false);
				}
			}
			Ok(true)
		}
		.await;
		access.set_node(node);
		access.subjects = previous;
		return Err(TaskModelNotApproved {
			model: selection.model.clone(),
			can_approve: allowed?,
		}
		.into());
	}
	match crate::authorization::catalog::entry(access, &selection.model, "model.infer").await {
		Ok(_) => {
			let config = aidash_domain::model::ModelConfig::from_stored(model.config)?;
			match crate::apps::identity::repositories::provider_credentials::check_task_route(
				&mut *access.tx,
				store,
				&access.identity.tenant,
				&access.identity.subject,
				&config,
				selection.reasoning_effort,
				remote,
			)
			.await
			{
				Err(e @ (Error::Invalid(_) | Error::Plan(_))) => Err(TaskModelUnusable {
					reasons: vec![e.to_string()],
				}
				.into()),
				result => result,
			}
		}
		Err(Error::Forbidden | Error::AuthorityRefused(_)) => Err(TaskModelUnusable {
			reasons: vec!["The current authority cannot use this model".into()],
		}
		.into()),
		Err(error) => Err(error),
	}
}

pub(crate) async fn authorized(
	task_model: Option<&aidash_domain::TaskModel>,
	access: &mut crate::authorization::access::Access,
	entry: &aidash_domain::registry::Entry,
	remote: bool,
	configured: bool,
) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
	authorized_mode(access, entry, remote, configured, false, None, task_model).await
}

/// [`authorized`] for a child agent's closure prepared under its delegating
/// Run's lease, before child admission appends the child to the subject
/// chain: the child's own direct dependencies are checked against the child,
/// not against the delegating agent that still ends the chain.
pub(crate) async fn authorized_child(
	selection: Option<&aidash_domain::TaskModel>,
	access: &mut crate::authorization::access::Access,
	entry: &aidash_domain::registry::Entry,
	configured: bool,
) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
	authorized_mode(access, entry, false, configured, true, None, selection).await
}

/// Resolve the receiver closure while retaining the failed dependency for refusal auditing.
pub(crate) async fn authorized_peer(
	selection: Option<&aidash_domain::TaskModel>,
	access: &mut crate::authorization::access::Access,
	entry: &aidash_domain::registry::Entry,
	configured: bool,
	failure: &mut Option<(aidash_domain::registry::EntityRef, u64)>,
) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
	authorized_mode(
		access,
		entry,
		true,
		configured,
		false,
		Some(failure),
		selection,
	)
	.await
}

async fn authorized_mode(
	access: &mut crate::authorization::access::Access,
	entry: &aidash_domain::registry::Entry,
	remote: bool,
	configured: bool,
	child: bool,
	mut failure: Option<&mut Option<(aidash_domain::registry::EntityRef, u64)>>,
	task_model: Option<&aidash_domain::TaskModel>,
) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
	let node = access.node_id.clone();
	let snapshot = snapshot_mode(
		&mut access.tx,
		&node,
		entry,
		remote,
		false,
		configured,
		task_model,
	)
	.await
	.map_err(|error| match error {
		Error::Invalid(reason) if entry.config["model"].is_null() && task_model.is_some() => {
			aidash_domain::TaskModelRefusal::TaskModelUnusable {
				reasons: vec![reason],
			}
			.into()
		}
		other => other,
	})?;
	if snapshot.task_model.is_some() {
		crate::bootstrap::registry_validation()
			.with_provider_credentials(configured)
			.bound_prompt_headroom(&snapshot, &serde_json::Value::Null)
			.map_err(|error| aidash_domain::TaskModelRefusal::TaskModelUnusable {
				reasons: vec![error.to_string()],
			})?;
	}
	// A delegated agent's own dependencies are checked against that agent.
	let delegated = snapshot.delegated_users()?;
	let root = [snapshot.agent.local()];
	for definition in &snapshot.definitions {
		if definition.identity.registry_deployment != node {
			return Err(Error::Forbidden);
		}
		if aidash_domain::registry::bindings::sources::is_selection_decider(&definition.definition)
		{
			// Metadata disclosure remains policy-controlled. Catalog Approval
			// for sending classification state is checked only when needed.
			let resource = crate::authorization::catalog::resource(access, &definition.definition);
			access.require(&resource, "registry.read").await?;
			continue;
		}
		let users = match delegated.get(&definition.identity) {
			Some(users) => users.iter().map(|user| user.local()).collect::<Vec<_>>(),
			// The root agent itself is used by the delegating subject.
			None if child && definition.identity != snapshot.agent => root.to_vec(),
			None => vec![],
		};
		let reference = definition.identity.local();
		let serial = access.decision_serial;
		let mut current = match crate::authorization::catalog::closure_entry(
			access,
			&reference,
			"registry.read",
			&users,
		)
		.await
		{
			Ok(current) => current,
			Err(error) => {
				if let Some(failure) = failure.as_deref_mut() {
					*failure = Some((reference, serial));
				}
				return Err(error);
			}
		};
		// A Built-in agent is stored without binding normalization; admission
		// normalized the same bindings, so compare it in that form (#1155).
		if current.binding_normalization.is_none() && definition.identity == snapshot.agent {
			current.binding_normalization = definition.definition.binding_normalization.clone();
		}
		if aidash_domain::registry::rules::digest(&serde_json::to_value(&current)?)
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
	configured: bool,
) -> aidash_application::Result<aidash_domain::registry::bindings::BindingSnapshot> {
	preview_mode(tx, node, entry, false, configured).await
}
async fn preview_mode(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	node: &str,
	entry: &aidash_domain::registry::Entry,
	remote: bool,
	configured: bool,
) -> aidash_application::Result<aidash_domain::registry::bindings::BindingSnapshot> {
	aidash_application::registry::bindings::resolve(
		&mut aidash_application::registry::bindings::catalog::LocalCatalog(
			aidash_application::registry::bindings::catalog::LookupCatalog {
				definitions: &mut super::NativeScope(tx),
				node,
			},
		),
		&crate::bootstrap::registry_validation().with_provider_credentials(configured),
		aidash_domain::registry::bindings::QualifiedRef {
			registry_deployment: node.into(),
			id: entry.id.clone(),
			version: entry.version.clone(),
		},
		entry,
		remote,
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
	if snapshot.agent.registry_deployment != access.node_id {
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
	let expected = crate::bootstrap::pinned_system_declaration(node, reference)?
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
