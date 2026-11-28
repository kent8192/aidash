//! Scratch Space persistence on a caller-owned Tenant transaction.
use std::collections::{BTreeMap, BTreeSet};

use aidash_domain::{
	Run,
	nodes::NodeName,
	policy::SubjectKind,
	scratch::{HostAllowlist, ScratchCeilings, ScratchSettings, ScratchSpace, ScratchState},
};
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{Model, execution::convert_values},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::Value;
use uuid::Uuid;

use crate::apps::workspaces::models::{
	ScratchSpace as SpaceRecord, ScratchSpaceMember as MemberRecord,
	ScratchSpaceSettings as SettingsRecord,
};
use crate::{
	Error, Result,
	apps::{execution::repositories::egress, identity::services::access::Access},
	store::Store,
};

/// Resolve an operator-named Space before opening its Tenant transaction.
pub(crate) async fn tenant_by_id(pool: &crate::database::native::Pool, id: Uuid) -> Result<String> {
	use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
	let scope = TenancyScope::Deployment(DeploymentScopeReason::ScratchSpaceTenant);
	let tenant: Option<String> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("tenant"))
			.from(Alias::new("scratch_spaces"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(pool.in_scope(&scope))
	.await?;
	tenant.ok_or_else(|| Error::NotFound("Scratch Space".into()))
}

/// Locks the row before loading dependent settings and members. The Tenant is
/// always the caller's scoped Tenant, never supplied by an API request.
pub(crate) async fn load(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
) -> Result<Option<ScratchSpace>> {
	load_with_lock(tx, tenant, id, Some(LockType::NoKeyUpdate)).await
}

/// Reads without a lock, for a check that decides nothing about the row's future.
pub(crate) async fn read(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
) -> Result<Option<ScratchSpace>> {
	load_with_lock(tx, tenant, id, None).await
}

/// Hold membership, state and settings stable until the caller commits.
pub(crate) async fn hold_for_use(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
) -> Result<Option<ScratchSpace>> {
	load_with_lock(tx, tenant, id, Some(LockType::Share)).await
}

async fn load_with_lock(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
	lock: Option<LockType>,
) -> Result<Option<ScratchSpace>> {
	let mut query = Query::select();
	query
		.columns(
			[
				"id",
				"is_default",
				"node",
				"name",
				"creator",
				"state",
				"revision",
				"wipe_generation",
				"wipe_completed_generation",
				"created_at",
				"last_used_at",
			]
			.map(Alias::new),
		)
		.from(Alias::new("scratch_spaces"))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant.to_owned())))
		.and_where(Expr::col("id").eq(Expr::value(id)));
	if let Some(lock) = lock {
		query.lock(lock);
	}
	let (sql, values) = query.build(PostgresQueryBuilder);
	let Some(row) = tx.fetch_optional(&sql, convert_values(values)).await? else {
		return Ok(None);
	};
	let err = reinhardt::core::exception::Error::from;
	let node: String = row.get("node").map_err(err)?;
	let state: String = row.get("state").map_err(err)?;
	let settings = crate::apps::workspaces::models::ScratchSpaceSettings::objects()
		.filter(crate::apps::workspaces::models::ScratchSpaceSettings::field_tenant().eq(tenant))
		.filter(crate::apps::workspaces::models::ScratchSpaceSettings::field_space_id().eq(id))
		.all_with_executor(tx)
		.await
		.map_err(err)?
		.pop()
		.ok_or_else(|| Error::NotFound("Scratch Space settings".into()))?
		.settings
		.0;
	let members: BTreeSet<String> = MemberRecord::objects()
		.filter(MemberRecord::field_tenant().eq(tenant))
		.filter(MemberRecord::field_space_id().eq(id))
		.all_with_executor(tx)
		.await
		.map_err(err)?
		.into_iter()
		.map(|row| row.subject)
		.collect();
	Ok(Some(ScratchSpace {
		is_default: row.get("is_default").map_err(err)?,
		id,
		node: node.parse()?,
		name: row.get("name").map_err(err)?,
		creator: row.get("creator").map_err(err)?,
		state: serde_json::from_value::<ScratchState>(Value::String(state))?,
		revision: row.get("revision").map_err(err)?,
		wipe_generation: row.get("wipe_generation").map_err(err)?,
		wipe_completed_generation: row.get("wipe_completed_generation").map_err(err)?,
		settings: serde_json::from_value::<ScratchSettings>(settings)?,
		members,
		created_at: row.get("created_at").map_err(err)?,
		last_used_at: row.get("last_used_at").map_err(err)?,
	}))
}

/// Insert a space and its settings/members in the same transaction.
pub(crate) async fn insert(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	space: &ScratchSpace,
) -> Result<()> {
	let err = reinhardt::core::exception::Error::from;
	let state = serde_json::to_value(space.state)?
		.as_str()
		.ok_or_else(|| Error::Invalid("invalid Scratch Space state".into()))?
		.to_owned();
	let row = SpaceRecord::build()
		.is_default(space.is_default)
		.id(space.id)
		.tenant(tenant.to_owned())
		.node(space.node.as_str().to_owned())
		.name(space.name.clone())
		.creator(space.creator.clone())
		.state(state)
		.revision(space.revision)
		.wipe_generation(space.wipe_generation)
		.wipe_completed_generation(space.wipe_completed_generation)
		.created_at(space.created_at)
		.last_used_at(space.last_used_at)
		.finish();
	SpaceRecord::objects()
		.insert_with_executor(tx, &row)
		.await
		.map_err(err)?;
	let settings = SettingsRecord::build()
		.space_id(space.id)
		.tenant(tenant.to_owned())
		.settings(reinhardt::db::orm::Json(serde_json::to_value(
			&space.settings,
		)?))
		.finish();
	SettingsRecord::objects()
		.insert_with_executor(tx, &settings)
		.await
		.map_err(err)?;
	write_members(tx, tenant, space).await
}

async fn write_members(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	space: &ScratchSpace,
) -> Result<()> {
	let err = reinhardt::core::exception::Error::from;
	let rows = MemberRecord::objects()
		.filter(MemberRecord::field_tenant().eq(tenant))
		.filter(MemberRecord::field_space_id().eq(space.id))
		.all_with_executor(tx)
		.await
		.map_err(err)?;
	let existing: BTreeSet<_> = rows.iter().map(|row| row.subject.clone()).collect();
	for row in rows {
		if !space.members.contains(&row.subject) {
			MemberRecord::objects()
				.delete_with_executor(tx, row.id)
				.await
				.map_err(err)?;
		}
	}
	for subject in space.members.difference(&existing) {
		let row = MemberRecord::build()
			.id(Uuid::new_v4())
			.tenant(tenant.to_owned())
			.space_id(space.id)
			.subject(subject.clone())
			.finish();
		MemberRecord::objects()
			.insert_with_executor(tx, &row)
			.await
			.map_err(err)?;
	}
	Ok(())
}

/// Replace mutable fields while the caller holds the space row lock.
pub(crate) async fn save(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	space: &ScratchSpace,
) -> Result<()> {
	let err = reinhardt::core::exception::Error::from;
	let mut row = SpaceRecord::objects()
		.filter(SpaceRecord::field_tenant().eq(tenant))
		.filter(SpaceRecord::field_id().eq(space.id))
		.all_with_executor(tx)
		.await
		.map_err(err)?
		.pop()
		.ok_or_else(|| Error::NotFound("Scratch Space".into()))?;
	row.state = serde_json::to_value(space.state)?
		.as_str()
		.ok_or_else(|| Error::Invalid("invalid Scratch Space state".into()))?
		.to_owned();
	row.revision = space.revision;
	row.last_used_at = space.last_used_at;
	SpaceRecord::objects()
		.save_with_executor(tx, &row)
		.await
		.map_err(err)?;
	let mut settings = SettingsRecord::objects()
		.filter(SettingsRecord::field_tenant().eq(tenant))
		.filter(SettingsRecord::field_space_id().eq(space.id))
		.all_with_executor(tx)
		.await
		.map_err(err)?
		.pop()
		.ok_or_else(|| Error::NotFound("Scratch Space settings".into()))?;
	let updated = serde_json::to_value(&space.settings)?;
	if settings.settings.0 != updated {
		settings.settings = reinhardt::db::orm::Json(updated);
		SettingsRecord::objects()
			.save_with_executor(tx, &settings)
			.await
			.map_err(err)?;
	}
	write_members(tx, tenant, space).await
}

#[cfg(test)]
async fn ids_on_node(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
) -> Result<Vec<Uuid>> {
	let (sql, values) = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("scratch_spaces"))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant.to_owned())))
		.and_where(Expr::col("node").eq(Expr::value(node.to_owned())))
		.order_by(Alias::new("created_at"), reinhardt::query::Order::Asc)
		.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
		.build(PostgresQueryBuilder);
	let err = reinhardt::core::exception::Error::from;
	Ok(tx
		.fetch_all(&sql, convert_values(values))
		.await?
		.iter()
		.map(|row| row.get("id").map_err(err))
		.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Read only spaces visible to the caller, with bounded batch reads for their
/// settings and members. The SQL visibility filter precedes the limit.
async fn list_visible(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
	person: &str,
) -> Result<Vec<ScratchSpace>> {
	let membership = Query::select()
		.column(Alias::new("space_id"))
		.from(Alias::new("scratch_space_members"))
		.and_where(Expr::col("tenant").eq(tenant.to_owned()))
		.and_where(Expr::col("subject").eq(person.to_owned()))
		.to_owned();
	let (sql, values) = Query::select()
		.columns(
			[
				"id",
				"is_default",
				"node",
				"name",
				"creator",
				"state",
				"revision",
				"wipe_generation",
				"wipe_completed_generation",
				"created_at",
				"last_used_at",
			]
			.map(Alias::new),
		)
		.from(Alias::new("scratch_spaces"))
		.and_where(Expr::col("tenant").eq(tenant.to_owned()))
		.and_where(Expr::col("node").eq(node.to_owned()))
		.and_where(Expr::col("state").eq("active"))
		.and_where(
			Expr::col("is_default")
				.eq(true)
				.or(Expr::col("creator").eq(person.to_owned()))
				.or(Expr::col("id").in_subquery(membership)),
		)
		.order_by(Alias::new("is_default"), reinhardt::query::Order::Desc)
		.order_by(Alias::new("created_at"), reinhardt::query::Order::Desc)
		.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
		.limit(500)
		.build(PostgresQueryBuilder);
	let rows = tx.fetch_all(&sql, convert_values(values)).await?;
	if rows.is_empty() {
		return Ok(Vec::new());
	}
	let err = reinhardt::core::exception::Error::from;
	let ids: Vec<Uuid> = rows
		.iter()
		.map(|row| row.get("id").map_err(err))
		.collect::<std::result::Result<_, _>>()?;
	let settings = SettingsRecord::objects()
		.filter(SettingsRecord::field_tenant().eq(tenant))
		.filter(SettingsRecord::field_space_id().is_in(ids.clone()))
		.all_with_executor(tx)
		.await
		.map_err(err)?;
	let mut settings: BTreeMap<_, _> = settings
		.into_iter()
		.map(|row| (row.space_id, row.settings.0))
		.collect();
	let member_rows = MemberRecord::objects()
		.filter(MemberRecord::field_tenant().eq(tenant))
		.filter(MemberRecord::field_space_id().is_in(ids))
		.all_with_executor(tx)
		.await
		.map_err(err)?;
	let mut members: BTreeMap<Uuid, BTreeSet<String>> = BTreeMap::new();
	for row in member_rows {
		members.entry(row.space_id).or_default().insert(row.subject);
	}
	let mut spaces = Vec::with_capacity(rows.len());
	for row in rows {
		let id: Uuid = row.get("id").map_err(err)?;
		let node: String = row.get("node").map_err(err)?;
		let state: String = row.get("state").map_err(err)?;
		spaces.push(ScratchSpace {
			is_default: row.get("is_default").map_err(err)?,
			id,
			node: node.parse()?,
			name: row.get("name").map_err(err)?,
			creator: row.get("creator").map_err(err)?,
			state: serde_json::from_value(Value::String(state))?,
			revision: row.get("revision").map_err(err)?,
			wipe_generation: row.get("wipe_generation").map_err(err)?,
			wipe_completed_generation: row.get("wipe_completed_generation").map_err(err)?,
			settings: serde_json::from_value(
				settings
					.remove(&id)
					.ok_or_else(|| Error::NotFound("Scratch Space settings".into()))?,
			)?,
			members: members.remove(&id).unwrap_or_default(),
			created_at: row.get("created_at").map_err(err)?,
			last_used_at: row.get("last_used_at").map_err(err)?,
		});
	}
	Ok(spaces)
}

/// Touch the space at most once per minute without locking a foreign key.
async fn touch(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
	now: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
	let (sql, values) = Query::update()
		.table(Alias::new("scratch_spaces"))
		.value(Alias::new("last_used_at"), now)
		.and_where(Expr::col("tenant").eq(tenant.to_owned()))
		.and_where(Expr::col("id").eq(Expr::value(id)))
		.and_where(Expr::col("state").eq("active"))
		.and_where(Expr::col("last_used_at").lt(Expr::value(now - chrono::Duration::minutes(1))))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

pub(crate) async fn delete(tx: &mut dyn TransactionExecutor, tenant: &str, id: Uuid) -> Result<()> {
	let err = reinhardt::core::exception::Error::from;
	let found = SpaceRecord::objects()
		.filter(SpaceRecord::field_tenant().eq(tenant))
		.filter(SpaceRecord::field_id().eq(id))
		.select_for_update()
		.all_with_executor(tx)
		.await
		.map_err(err)?;
	if found.is_empty() {
		return Err(Error::NotFound("Scratch Space".into()));
	}
	SpaceRecord::objects()
		.delete_with_executor(tx, id)
		.await
		.map_err(err)?;
	Ok(())
}

/// The Home person a Run executing here for another Home acts for: the credential's subject, or
/// for a Node of the Run's own Tenant the root of the chain its admission recorded (#751). It
/// keys the Delegated Run Scratch Space and the receiving Working Area, so both belong to one
/// person. Only this Run's own admission can name it, and the person must be on the caller's
/// chain.
pub(crate) async fn remote_person(
	access: &mut Access,
	run: &aidash_domain::RunMetadata,
) -> aidash_application::Result<String> {
	// A receiver may only allocate for its own already-admitted Run.
	// A shared lock keeps the admission from changing meanwhile without
	// blocking activation, which shares it too; the Node lock below
	// serializes first allocation.
	let tenant = access.identity.tenant.clone();
	let (sql, values) = Query::select()
		.column(Alias::new("credential_id"))
		.expr_as(
			Expr::cust("COALESCE(subject_chain[1], '')"),
			Alias::new("root_subject"),
		)
		.from(Alias::new("authorization_remote_admissions"))
		.and_where(Expr::col("id").eq(Expr::value(run.id)))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant.clone())))
		.and_where(Expr::col("node").eq(Expr::value(run.node.as_str().to_owned())))
		.and_where(Expr::col("task_id").eq(Expr::value(run.task_id)))
		.and_where(Expr::col("source_node").eq(Expr::value(run.home_node.clone())))
		.and_where(
			Expr::col("id").in_subquery(
				Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("task_executions"))
					.and_where(Expr::col("tenant").eq(Expr::value(tenant.clone())))
					.and_where(Expr::col("thread_id").eq(Expr::value(run.thread_id)))
					.and_where(Expr::col("home_node").eq(Expr::value(run.home_node.clone())))
					.to_owned(),
			),
		)
		.lock(LockType::Share)
		.build(PostgresQueryBuilder);
	let admission = access
		.tx
		.fetch_optional(&sql, convert_values(values))
		.await
		.map_err(crate::Error::from)?
		.ok_or(aidash_application::Error::Forbidden)?;
	let credential: Uuid = admission
		.get("credential_id")
		.map_err(reinhardt::core::exception::Error::from)
		.map_err(crate::Error::from)?;
	let person: String = if credential.is_nil() {
		// A Node of the Run's own Tenant acts for a subject of that Tenant
		// without a credential (ADR 0064), so the person who started the
		// work is the root of the chain the admission recorded (#751).
		let root: String = admission
			.get("root_subject")
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?;
		if root.is_empty() {
			return Err(aidash_application::Error::Forbidden);
		}
		root
	} else {
		let (sql, values) = Query::select()
			.column(Alias::new("subject"))
			.from(Alias::new("authorization_credentials"))
			.and_where(Expr::col("id").eq(Expr::value(credential)))
			.and_where(Expr::col("tenant").eq(tenant.clone()))
			.and_where(Expr::col("revoked_at").is_null())
			.and_where(Expr::col("expires_at").gt(Expr::value(chrono::Utc::now())))
			.lock(LockType::Share)
			.build(PostgresQueryBuilder);
		let row = access
			.tx
			.fetch_optional(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?
			.ok_or(aidash_application::Error::Forbidden)?;
		row.get("subject")
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?
	};
	if !access.subjects.contains(&person) {
		return Err(aidash_application::Error::Forbidden);
	}
	Ok(person)
}

/// Look up the Node's default space even after a Run loses authorization.
pub(crate) async fn default_space_id(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
) -> Result<Uuid> {
	let sql = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("scratch_spaces"))
		.and_where(Expr::col("tenant").eq(tenant.to_owned()))
		.and_where(Expr::col("node").eq(node.to_owned()))
		.and_where(Expr::col("is_default").eq(true))
		.to_string(PostgresQueryBuilder);
	crate::database::native::query_scalar(&sql)
		.scalar_optional(tx)
		.await?
		.ok_or_else(|| Error::NotFound("Node Scratch Space".into()))
}

pub(crate) async fn create_default(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &NodeName,
	now: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
	insert(
		tx,
		tenant,
		&ScratchSpace {
			id: Uuid::new_v4(),
			is_default: true,
			node: node.clone(),
			name: "Node Scratch Space".into(),
			creator: String::new(),
			members: BTreeSet::new(),
			state: ScratchState::Active,
			revision: 0,
			wipe_generation: 0,
			wipe_completed_generation: 0,
			settings: ScratchSettings::for_node(),
			created_at: now,
			last_used_at: now,
		},
	)
	.await
}

/// Tasks with active Runs affected by wiping this Node's default space.
/// Remote Runs always use the default. A local Run uses its Thread's binding.
pub(crate) async fn running_tasks_in_default(
	access: &mut Access,
	space: &ScratchSpace,
) -> Result<Vec<Uuid>> {
	let tenant = access.identity.tenant.clone();
	let bound_threads = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("threads"))
		.and_where(Expr::col("tenant").eq(tenant.clone()))
		.and_where(Expr::col("scratch_space_id").eq(Expr::value(space.id)))
		.to_owned();
	let sql = Query::select()
		.distinct()
		.column(Alias::new("task_id"))
		.from(Alias::new("task_executions"))
		.and_where(Expr::col("tenant").eq(tenant))
		.and_where(Expr::col("node").eq(space.node.as_str().to_owned()))
		.and_where(Expr::col("control").eq("ACTIVE"))
		.and_where(Expr::col("phase").is_in(["READY", "THINKING", "TOOL_CALL", "WAITING"]))
		.and_where(
			Expr::col("home_node")
				.ne(format!("{}/{}", access.node_id, space.node))
				.or(Expr::col("thread_id").in_subquery(bound_threads)),
		)
		.order_by(Alias::new("task_id"), reinhardt::query::Order::Asc)
		.to_string(PostgresQueryBuilder);
	Ok(crate::database::native::query_scalar(&sql)
		.scalar_all(&mut **access.tx)
		.await?)
}

/// Process one wipe while holding the row. The acknowledgment follows physical
/// deletion; a lost acknowledgment retries before this space can start a Pod.
pub(crate) async fn wipe_pending_space(store: &Store) -> Result<()> {
	use aidash_application::{
		ports::capabilities::execution_pod::ExecutionPodRuntime,
		tenancy::{DeploymentScopeReason, TenancyScope},
	};

	if store.capabilities.0.runner.is_none() {
		return Ok(());
	}
	let runner = crate::bootstrap::operation_runner(store)?;
	let mut tx = store
		.pool
		.begin(&TenancyScope::Deployment(
			DeploymentScopeReason::CapabilityMaintenance,
		))
		.await?;
	let (sql, values) = Query::select()
		.columns(["id", "tenant", "wipe_generation"].map(Alias::new))
		.from(Alias::new("scratch_spaces"))
		.and_where(Expr::col("wipe_generation").gt(Expr::col("wipe_completed_generation")))
		.order_by(Alias::new("last_used_at"), reinhardt::query::Order::Asc)
		.limit(1)
		.lock(LockType::NoKeyUpdate)
		.lock_behavior(reinhardt::query::LockBehavior::SkipLocked)
		.build(PostgresQueryBuilder);
	if let Some(row) = tx.fetch_optional(&sql, convert_values(values)).await? {
		let err = reinhardt::core::exception::Error::from;
		let id: Uuid = row.get("id").map_err(err)?;
		let tenant: String = row.get("tenant").map_err(err)?;
		let generation: i64 = row.get("wipe_generation").map_err(err)?;
		runner
			.delete(&id.to_string())
			.await
			.map_err(|error| Error::External(error.to_string()))?;
		let (sql, values) = Query::update()
			.table(Alias::new("scratch_spaces"))
			.value(Alias::new("wipe_completed_generation"), generation)
			.and_where(Expr::col("tenant").eq(tenant))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
	}
	tx.commit().await?;
	Ok(())
}

pub(crate) async fn request_thread_wipe(access: &mut Access, thread: Uuid) -> Result<()> {
	access.require_thread(thread, "thread.read").await?;
	access.require_thread(thread, "message.create").await?;
	let (sql, values) = Query::update()
		.table(Alias::new("threads"))
		.value(Alias::new("scratch_wipe_requested"), true)
		.and_where(Expr::col("tenant").eq(access.identity.tenant.clone()))
		.and_where(Expr::col("id").eq(Expr::value(thread)))
		.and_where(Expr::col("deleted_at").is_null())
		.build(PostgresQueryBuilder);
	if access
		.tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected
		!= 1
	{
		return Err(Error::NotFound("Thread".into()));
	}
	Ok(())
}

pub(crate) async fn wipe_pending_thread(store: &Store) -> Result<()> {
	use aidash_application::{
		ports::capabilities::execution_pod::ExecutionPodRuntime,
		tenancy::{DeploymentScopeReason, TenancyScope},
	};
	let Some(profile) = store.capabilities.0.runner.as_ref() else {
		return Ok(());
	};
	let Some(bound) = store.in_process.get() else {
		return Ok(());
	};
	let mut tx = store
		.pool
		.begin(&TenancyScope::Deployment(
			DeploymentScopeReason::CapabilityMaintenance,
		))
		.await?;
	let (sql, values) = Query::select()
		.columns(["id", "tenant", "scratch_space_id"].map(Alias::new))
		.expr_as(Expr::col("deleted_at").is_not_null(), Alias::new("deleted"))
		.from(Alias::new("threads"))
		.and_where(Expr::col("scratch_wipe_requested").eq(true))
		.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
		.limit(1)
		.lock(LockType::NoKeyUpdate)
		.lock_behavior(reinhardt::query::LockBehavior::SkipLocked)
		.build(PostgresQueryBuilder);
	if let Some(row) = tx.fetch_optional(&sql, convert_values(values)).await? {
		let err = reinhardt::core::exception::Error::from;
		let thread: Uuid = row.get("id").map_err(err)?;
		let tenant: String = row.get("tenant").map_err(err)?;
		let space_id: Uuid = row.get("scratch_space_id").map_err(err)?;
		let deleted: bool = row.get("deleted").map_err(err)?;
		let space = read(&mut *tx, &tenant, space_id)
			.await?
			.ok_or_else(|| Error::NotFound("Thread Scratch Space".into()))?;
		if space.wipe_pending() {
			return Ok(());
		}
		if space.state == ScratchState::Active {
			let settings = crate::apps::execution::services::execution_pod_command::pod_settings(
				&space,
				&tenant,
				&bound.config.scratch_policy,
				&profile.image,
			)?;
			let runner = crate::bootstrap::operation_runner(store)?;
			if deleted {
				// Deleting the Thread cancels its Runs in PostgreSQL. Fence
				// commands already on their way to this Pod before removing
				// the directory, so they cannot recreate it after the wipe.
				let sql = Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("task_executions"))
					.and_where(Expr::col("tenant").eq(tenant.clone()))
					.and_where(Expr::col("thread_id").eq(Expr::value(thread)))
					.and_where(Expr::col("node").eq(space.node.as_str().to_owned()))
					.to_string(PostgresQueryBuilder);
				let runs: Vec<Uuid> = crate::database::native::query_scalar(&sql)
					.scalar_all(&mut *tx)
					.await?;
				for run in runs {
					runner
						.stop_run(&space_id.to_string(), &run.to_string())
						.await
						.map_err(|error| Error::External(error.to_string()))?;
				}
			}
			runner
				.clear_thread(&settings, thread)
				.await
				.map_err(|error| Error::External(error.to_string()))?;
		}
		let (sql, values) = Query::update()
			.table(Alias::new("threads"))
			.value(Alias::new("scratch_wipe_requested"), false)
			.and_where(Expr::col("tenant").eq(tenant))
			.and_where(Expr::col("id").eq(Expr::value(thread)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
	}
	tx.commit().await?;
	Ok(())
}

/// Return a conservative set of the subjects who may see the destination.
/// None means a remote reader cannot be checked against a Tenant-local source.
pub(crate) async fn copy_audience(
	access: &mut Access,
	space: &ScratchSpace,
) -> Result<Option<BTreeSet<String>>> {
	if !space.is_default {
		let mut readers = space.members.clone();
		readers.insert(space.creator.clone());
		return Ok(Some(readers));
	}
	let tenant = access.identity.tenant.clone();
	let node = space.node.as_str().to_owned();
	let remote_mappings = Query::select()
		.expr(Expr::val(1))
		.from(Alias::new("authorization_peer_mappings"))
		.and_where(Expr::col("tenant").eq(tenant.clone()))
		.and_where(Expr::col("node").eq(node.clone()))
		.and_where(Expr::col("enabled").eq(true))
		.to_owned();
	let remote_admissions = Query::select()
		.expr(Expr::val(1))
		.from(Alias::new("authorization_remote_admissions"))
		.and_where(Expr::col("tenant").eq(tenant))
		.and_where(Expr::col("node").eq(node))
		.and_where(Expr::col("credential_id").ne(Expr::value(Uuid::nil())))
		.and_where(Expr::col("cancelled").eq(false))
		.to_owned();
	let has_remote_reader = Query::select()
		.expr(Expr::exists(remote_mappings).or(Expr::exists(remote_admissions)))
		.to_string(PostgresQueryBuilder);
	if crate::database::native::query_scalar::<bool>(&has_remote_reader)
		.scalar_one(&mut **access.tx)
		.await?
	{
		return Ok(None);
	}
	let resource = access.node_resource(&space.node).await?;
	// A condition that fails in this request may allow another request on
	// this Node. Overestimate the allowed audience before reading sources.
	let mut potential = access.snapshot.bundle.clone();
	potential
		.policies
		.retain(|p| p.effect == aidash_domain::policy::Effect::Allow);
	for policy in &mut potential.policies {
		policy.condition = None;
	}
	use aidash_domain::identity::tenant_administration::{NODE_ADMINISTER, NODE_USE};
	Ok(Some(
		potential
			.subjects
			.keys()
			.filter(|subject| {
				[NODE_USE, NODE_ADMINISTER].iter().any(|action| {
					potential
						.evaluate(&access.evaluation(subject, &resource, action))
						.allowed
				})
			})
			.cloned()
			.collect(),
	))
}

/// Inputs that depend on Deployment configuration, the Registry, and the
/// Execution Pod lifecycle. Implementations retain the caller's Access transaction.
#[async_trait::async_trait]
pub(crate) trait ScratchServices: Send {
	async fn limits(
		&mut self,
		access: &mut Access,
		node: &NodeName,
	) -> aidash_application::Result<(ScratchCeilings, HostAllowlist)>;
	async fn validate_references(
		&mut self,
		access: &mut Access,
		node: &NodeName,
		settings: &ScratchSettings,
	) -> aidash_application::Result<()>;
	async fn validate_runtime(
		&mut self,
		access: &mut Access,
		node: &NodeName,
		settings: &ScratchSettings,
	) -> aidash_application::Result<()>;
	async fn clear_contents(
		&mut self,
		access: &mut Access,
		id: Uuid,
	) -> aidash_application::Result<()>;
	async fn run_binding(
		&mut self,
		access: &mut Access,
		run: &Run,
	) -> aidash_application::Result<(Uuid, String)>;
}

pub(crate) fn effective_policy(
	policy: &crate::config::ScratchOperatorPolicy,
	tenant: &str,
) -> (ScratchCeilings, HostAllowlist) {
	let mut ceilings = policy.ceilings.clone();
	ceilings.docker_allowed |= policy.docker_tenants.contains(tenant);
	let hosts = policy.tenant_hosts.get(tenant).cloned().unwrap_or_else(|| {
		aidash_domain::scratch::default_public_hosts()
			.into_iter()
			.collect()
	});
	let allowed = if hosts.len() == 1 && hosts[0] == "*" {
		HostAllowlist::All
	} else {
		HostAllowlist::Hosts(hosts.into_iter().collect())
	};
	(ceilings, allowed)
}

/// Uses the same Access transaction for policy checks, audit and persistence.
pub(crate) struct ScratchAdapter<'a> {
	pub access: &'a mut Access,
	pub services: &'a mut dyn ScratchServices,
	pub person_override: Option<String>,
}

impl ScratchAdapter<'_> {
	/// Users and services may own spaces. An Agent may own only the space
	/// allocated for its delegated Peer Mapping credential.
	fn may_own(&self, person: &str) -> bool {
		let delegated = self.person_override.is_some();
		self.access
			.snapshot
			.bundle
			.subjects
			.get(person)
			.is_some_and(|subject| {
				subject.enabled
					&& match subject.kind {
						SubjectKind::User | SubjectKind::Service => true,
						SubjectKind::Agent => delegated,
					}
			})
	}
}

#[async_trait::async_trait]
impl aidash_application::scratch::ScratchScope for ScratchAdapter<'_> {
	fn person(&self) -> &str {
		self.person_override
			.as_deref()
			.unwrap_or(&self.access.identity.subject)
	}
	async fn default_space(&mut self, node: &NodeName) -> aidash_application::Result<Uuid> {
		let tenant = self.access.identity.tenant.clone();
		default_space_id(&mut **self.access.tx, &tenant, node.as_str())
			.await
			.map_err(Into::into)
	}
	async fn require_node_admin(&mut self, node: &NodeName) -> aidash_application::Result<()> {
		let resource = self.access.node_resource(node).await?;
		self.access
			.require(
				&resource,
				aidash_domain::identity::tenant_administration::NODE_ADMINISTER,
			)
			.await
			.map_err(Into::into)
	}

	async fn require_create(&mut self, node: &NodeName) -> aidash_application::Result<()> {
		let resource = self.access.node_resource(node).await?;
		let person = self
			.person_override
			.clone()
			.unwrap_or_else(|| self.access.identity.subject.clone());
		let evaluation = self.access.evaluation(
			&person,
			&resource,
			aidash_domain::identity::tenant_administration::SCRATCH_CREATE,
		);
		let mut decision = self.access.snapshot.bundle.evaluate(&evaluation);
		decision.revision = self.access.snapshot.revision;
		self.access
			.record(&[(evaluation, decision.clone())])
			.await?;
		if !decision.allowed {
			return Err(aidash_application::Error::Forbidden);
		}
		Ok(())
	}
	async fn require_node_use(
		&mut self,
		node: &NodeName,
		person: &str,
	) -> aidash_application::Result<()> {
		let resource = self.access.node_resource(node).await?;
		use aidash_domain::identity::tenant_administration::{NODE_ADMINISTER, NODE_USE};
		if !self.may_own(person) {
			return Err(aidash_application::Error::Forbidden);
		}
		if !self.access.decide(&resource, NODE_USE).await?
			&& !self.access.decide(&resource, NODE_ADMINISTER).await?
		{
			return Err(aidash_application::Error::Forbidden);
		}
		for action in [NODE_USE, NODE_ADMINISTER] {
			let evaluation = self.access.evaluation(person, &resource, action);
			let mut decision = self.access.snapshot.bundle.evaluate(&evaluation);
			decision.revision = self.access.snapshot.revision;
			let allowed = decision.allowed;
			self.access.record(&[(evaluation, decision)]).await?;
			if allowed {
				self.access.set_node(node.clone());
				return Ok(());
			}
		}
		Err(aidash_application::Error::Forbidden)
	}

	async fn limits(
		&mut self,
		node: &NodeName,
	) -> aidash_application::Result<(ScratchCeilings, HostAllowlist)> {
		self.services.limits(self.access, node).await
	}
	async fn validate_references(
		&mut self,
		node: &NodeName,
		settings: &ScratchSettings,
	) -> aidash_application::Result<()> {
		self.services
			.validate_references(self.access, node, settings)
			.await
	}
	async fn validate_runtime(
		&mut self,
		node: &NodeName,
		settings: &ScratchSettings,
	) -> aidash_application::Result<()> {
		self.services
			.validate_runtime(self.access, node, settings)
			.await
	}
	async fn validate_owner(&mut self, person: &str) -> aidash_application::Result<()> {
		if !self.may_own(person) {
			return Err(aidash_application::Error::Forbidden);
		}
		Ok(())
	}
	async fn validate_members(
		&mut self,
		members: &BTreeSet<String>,
	) -> aidash_application::Result<()> {
		use aidash_domain::policy::SubjectKind;
		if members.iter().any(|member| {
			!self
				.access
				.snapshot
				.bundle
				.subjects
				.get(member)
				.is_some_and(|subject| subject.kind == SubjectKind::User && subject.enabled)
		}) {
			return Err(aidash_application::Error::Forbidden);
		}
		Ok(())
	}
	async fn insert(&mut self, space: &ScratchSpace) -> aidash_application::Result<()> {
		let tenant = self.access.identity.tenant.clone();
		insert(&mut **self.access.tx, &tenant, space)
			.await
			.map_err(Into::into)
	}
	async fn load_for_update(&mut self, id: Uuid) -> aidash_application::Result<ScratchSpace> {
		let tenant = self.access.identity.tenant.clone();
		load(&mut **self.access.tx, &tenant, id)
			.await
			.map_err(aidash_application::Error::from)?
			.ok_or_else(|| aidash_application::Error::NotFound("Scratch Space".into()))
	}
	async fn load(&mut self, id: Uuid) -> aidash_application::Result<ScratchSpace> {
		let tenant = self.access.identity.tenant.clone();
		load_with_lock(&mut **self.access.tx, &tenant, id, None)
			.await
			.map_err(aidash_application::Error::from)?
			.ok_or_else(|| aidash_application::Error::NotFound("Scratch Space".into()))
	}
	async fn list(&mut self, node: &NodeName) -> aidash_application::Result<Vec<ScratchSpace>> {
		let tenant = self.access.identity.tenant.clone();
		let person = self.person().to_owned();
		list_visible(&mut **self.access.tx, &tenant, node.as_str(), &person)
			.await
			.map_err(Into::into)
	}
	async fn save(&mut self, space: &ScratchSpace) -> aidash_application::Result<()> {
		let tenant = self.access.identity.tenant.clone();
		save(&mut **self.access.tx, &tenant, space)
			.await
			.map_err(Into::into)
	}
	async fn touch(
		&mut self,
		id: Uuid,
		now: chrono::DateTime<chrono::Utc>,
	) -> aidash_application::Result<()> {
		let tenant = self.access.identity.tenant.clone();
		touch(&mut **self.access.tx, &tenant, id, now)
			.await
			.map_err(Into::into)
	}
	async fn delete(&mut self, id: Uuid) -> aidash_application::Result<()> {
		let tenant = self.access.identity.tenant.clone();
		delete(&mut **self.access.tx, &tenant, id)
			.await
			.map_err(Into::into)
	}
	async fn clear_contents(&mut self, id: Uuid) -> aidash_application::Result<()> {
		self.services.clear_contents(self.access, id).await
	}
	async fn run_binding(&mut self, run: &Run) -> aidash_application::Result<(Uuid, String)> {
		self.services.run_binding(self.access, run).await
	}
}

/// Deployment inputs for Scratch Space admission.
pub(crate) struct NativeScratchServices<'a> {
	pub policy: &'a crate::config::ScratchOperatorPolicy,
}

#[async_trait::async_trait]
impl ScratchServices for NativeScratchServices<'_> {
	async fn limits(
		&mut self,
		access: &mut Access,
		_: &NodeName,
	) -> aidash_application::Result<(ScratchCeilings, HostAllowlist)> {
		Ok(effective_policy(self.policy, &access.identity.tenant))
	}

	async fn validate_runtime(
		&mut self,
		access: &mut Access,
		_: &NodeName,
		settings: &ScratchSettings,
	) -> aidash_application::Result<()> {
		if let Some(image) = &settings.execution_image
			&& !self
				.policy
				.tenant_images
				.get(&access.identity.tenant)
				.is_some_and(|images| images.contains(image))
		{
			return Err(aidash_application::Error::Forbidden);
		}
		Ok(())
	}

	async fn validate_references(
		&mut self,
		access: &mut Access,
		node: &NodeName,
		settings: &ScratchSettings,
	) -> aidash_application::Result<()> {
		self.validate_runtime(access, node, settings).await?;
		// A service is added only when a Catalog Approval for this Node, not limited
		// to certain subjects or Agents, covers it (#738). Services already on the
		// Space are not passed here, so a later withdrawal never blocks a save.
		let tenant = access.identity.tenant.clone();
		let usable = egress::usable_services(
			&mut access.tx,
			&tenant,
			node.as_str(),
			&settings.external_services,
		)
		.await
		.map_err(aidash_application::Error::from)?;
		for (id, result) in usable {
			match result {
				Ok(entry) => {
					// Whoever adds it must still be able to read the definition. The
					// approval was checked on the Space's Node above, so this asks
					// policy alone rather than the caller's own Node.
					let resource = crate::authorization::catalog::resource(access, &entry);
					if !access
						.decide(&resource, "registry.read")
						.await
						.map_err(aidash_application::Error::from)?
					{
						return Err(aidash_application::Error::Forbidden);
					}
					aidash_domain::egress::definition(&entry)?;
				}
				Err(reason) => return Err(refusal(&id, reason)),
			}
		}
		Ok(())
	}

	async fn clear_contents(
		&mut self,
		access: &mut Access,
		id: Uuid,
	) -> aidash_application::Result<()> {
		// Each request has a distinct durable generation so a worker cannot
		// acknowledge a later wipe while processing an earlier one.
		let tenant = access.identity.tenant.clone();
		let (sql, values) = Query::update()
			.table(Alias::new("scratch_spaces"))
			.value_expr(
				Alias::new("wipe_generation"),
				Expr::col("wipe_generation").add(1),
			)
			.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.build(PostgresQueryBuilder);
		let result = access
			.tx
			.execute(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?;
		if result.rows_affected != 1 {
			return Err(aidash_application::Error::Forbidden);
		}
		Ok(())
	}

	async fn run_binding(
		&mut self,
		access: &mut Access,
		run: &Run,
	) -> aidash_application::Result<(Uuid, String)> {
		self.run_binding_metadata(access, &run.metadata()).await
	}
}

impl NativeScratchServices<'_> {
	pub(crate) async fn run_binding_metadata(
		&mut self,
		access: &mut Access,
		run: &aidash_domain::RunMetadata,
	) -> aidash_application::Result<(Uuid, String)> {
		if !run.is_local(&access.node_id) {
			let person = remote_person(access, run).await?;
			let id = default_space_id(&mut **access.tx, &access.identity.tenant, run.node.as_str())
				.await
				.map_err(aidash_application::Error::from)?;
			return Ok((id, person));
		}

		let (sql, values) = Query::select()
			.columns([Alias::new("scratch_space_id"), Alias::new("created_by")])
			.from(Alias::new("threads"))
			.and_where(Expr::col("id").eq(Expr::value(run.thread_id)))
			.and_where(Expr::col("tenant").eq(Expr::value(access.identity.tenant.clone())))
			.and_where(Expr::col("node").eq(Expr::value(run.node.as_str().to_owned())))
			.build(PostgresQueryBuilder);
		let row = access
			.tx
			.fetch_optional(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?
			.ok_or(aidash_application::Error::Forbidden)?;
		let err = reinhardt::core::exception::Error::from;
		let id = row
			.get("scratch_space_id")
			.map_err(err)
			.map_err(crate::Error::from)?;
		let starter: String = row
			.get("created_by")
			.map_err(err)
			.map_err(crate::Error::from)?;
		if !access
			.snapshot
			.bundle
			.subjects
			.get(&starter)
			.is_some_and(|subject| subject.enabled)
		{
			return Err(aidash_application::Error::Forbidden);
		}
		Ok((id, starter))
	}
}

#[cfg(test)]
mod tests;

/// The refusal for adding an External service that is not usable now.
fn refusal(id: &str, reason: &str) -> aidash_application::Error {
	use aidash_domain::egress::SERVICE_APPROVAL_LIMITED;
	aidash_application::Error::Invalid(if reason == SERVICE_APPROVAL_LIMITED {
		format!(
			"External service {id} is approved only for certain users or Agents; a Scratch Space needs a Catalog Approval for everyone on this Node"
		)
	} else {
		format!(
			"External service {id} has no Catalog Approval for everyone on this Node, or it is not registered"
		)
	})
}

/// Whether an External service is registered for the Tenant at any version.
async fn registered_service(access: &mut Access, id: &str) -> Result<bool> {
	let (sql, values) = Query::select()
		.column(Alias::new("version"))
		.from(Alias::new("registry"))
		.and_where(Expr::col("tenant").eq(Expr::value(access.identity.tenant.clone())))
		.and_where(Expr::col("id").eq(Expr::value(id.to_owned())))
		.and_where(Expr::col("kind").eq(Expr::value("external_service")))
		.build(PostgresQueryBuilder);
	Ok(!access
		.tx
		.fetch_all(&sql, convert_values(values))
		.await
		.map_err(crate::Error::from)?
		.is_empty())
}

/// Whether one External service on a Scratch Space is usable now.
pub(crate) struct ServiceState {
	pub id: String,
	pub usable: bool,
	pub reason: Option<&'static str>,
}

/// The state of every External service on `space`, for its settings display.
pub(crate) async fn service_states(
	store: &Store,
	access: &mut Access,
	space: &ScratchSpace,
) -> Result<Vec<ServiceState>> {
	let tenant = access.identity.tenant.clone();
	let usable = egress::usable_services(
		&mut access.tx,
		&tenant,
		space.node.as_str(),
		&space.settings.external_services,
	)
	.await?;
	let mut states = Vec::new();
	for (id, result) in usable {
		let reason = match result {
			Ok(entry) => {
				let exceeds_limits =
					aidash_domain::egress::definition(&entry).is_ok_and(|definition| {
						egress::check_service_limits(store, &definition).is_err()
					});
				exceeds_limits.then_some("limits_exceeded")
			}
			Err(reason) if reason == aidash_domain::egress::SERVICE_APPROVAL_LIMITED => {
				Some("approval_limited")
			}
			Err(_) if !registered_service(access, &id).await? => Some("not_found"),
			Err(_) => Some("not_approved"),
		};
		states.push(ServiceState {
			id,
			usable: reason.is_none(),
			reason,
		});
	}
	Ok(states)
}

/// The External services of a Run's Scratch Space that are usable now, as its
/// starting instructions list them. Empty when the Space is gone or not Active.
pub(crate) async fn usable_lines(
	store: &Store,
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	space: Uuid,
) -> Result<Vec<aidash_application::ports::execution::ExternalServiceLine>> {
	let Some(space) = read(&mut **tx, tenant, space).await? else {
		return Ok(Vec::new());
	};
	if space.state != aidash_domain::scratch::ScratchState::Active {
		return Ok(Vec::new());
	}
	let usable = egress::usable_services(
		tx,
		tenant,
		space.node.as_str(),
		&space.settings.external_services,
	)
	.await?;
	let mut lines = Vec::new();
	for (id, result) in usable {
		let Ok(entry) = result else { continue };
		let Ok(definition) = aidash_domain::egress::definition(&entry) else {
			continue;
		};
		if egress::check_service_limits(store, &definition).is_err() {
			continue;
		}
		let description = entry
			.description
			.get("en")
			.or_else(|| entry.description.values().next())
			.cloned()
			.unwrap_or_default();
		lines.push(aidash_application::ports::execution::ExternalServiceLine {
			url: format!("http://{}/{id}/", aidash_domain::egress::SERVICES_HOST),
			id,
			methods: definition.methods,
			description,
		});
	}
	Ok(lines)
}
