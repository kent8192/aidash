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
use crate::{Error, Result, apps::identity::services::access::Access};

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
	load_with_lock(tx, tenant, id, true).await
}

/// Reads without a lock, for a check that decides nothing about the row's future.
pub(crate) async fn read(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
) -> Result<Option<ScratchSpace>> {
	load_with_lock(tx, tenant, id, false).await
}

async fn load_with_lock(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
	lock: bool,
) -> Result<Option<ScratchSpace>> {
	let mut query = Query::select();
	query
		.columns(
			[
				"id",
				"node",
				"name",
				"creator",
				"state",
				"revision",
				"wipe_generation",
				"created_at",
				"last_used_at",
			]
			.map(Alias::new),
		)
		.from(Alias::new("scratch_spaces"))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant.to_owned())))
		.and_where(Expr::col("id").eq(Expr::value(id)));
	if lock {
		query.lock(LockType::NoKeyUpdate);
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
		id,
		node: node.parse()?,
		name: row.get("name").map_err(err)?,
		creator: row.get("creator").map_err(err)?,
		state: serde_json::from_value::<ScratchState>(Value::String(state))?,
		revision: row.get("revision").map_err(err)?,
		wipe_generation: row.get("wipe_generation").map_err(err)?,
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
		.id(space.id)
		.tenant(tenant.to_owned())
		.node(space.node.as_str().to_owned())
		.name(space.name.clone())
		.creator(space.creator.clone())
		.state(state)
		.revision(space.revision)
		.wipe_generation(space.wipe_generation)
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
				"node",
				"name",
				"creator",
				"state",
				"revision",
				"wipe_generation",
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
			Expr::col("creator")
				.eq(person.to_owned())
				.or(Expr::col("id").in_subquery(membership)),
		)
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
			id,
			node: node.parse()?,
			name: row.get("name").map_err(err)?,
			creator: row.get("creator").map_err(err)?,
			state: serde_json::from_value(Value::String(state))?,
			revision: row.get("revision").map_err(err)?,
			wipe_generation: row.get("wipe_generation").map_err(err)?,
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

/// A receiving Node keeps one space per home Workspace and mapped person.
fn remote_space_id(
	tenant: &str,
	node: &NodeName,
	home: &str,
	workspace: Uuid,
	person: &str,
) -> Uuid {
	use sha2::{Digest, Sha256};
	let mut hash = Sha256::new();
	hash.update(b"aidash:remote-scratch-space:v1");
	for value in [tenant, node.as_str(), home, person] {
		hash.update((value.len() as u64).to_be_bytes());
		hash.update(value.as_bytes());
	}
	hash.update(workspace.as_bytes());
	let digest = hash.finalize();
	let mut bytes = [0_u8; 16];
	bytes.copy_from_slice(&digest[..16]);
	bytes[6] = (bytes[6] & 0x0f) | 0x80;
	bytes[8] = (bytes[8] & 0x3f) | 0x80;
	Uuid::from_bytes(bytes)
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
		let action = aidash_domain::identity::tenant_administration::NODE_USE;
		self.access.require(&resource, action).await?;
		if !self.may_own(person) {
			return Err(aidash_application::Error::Forbidden);
		}
		{
			let evaluation = self.access.evaluation(person, &resource, action);
			let mut decision = self.access.snapshot.bundle.evaluate(&evaluation);
			decision.revision = self.access.snapshot.revision;
			self.access
				.record(&[(evaluation, decision.clone())])
				.await?;
			if !decision.allowed {
				return Err(aidash_application::Error::Forbidden);
			}
		}
		self.access.set_node(node.clone());
		Ok(())
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
		load_with_lock(&mut **self.access.tx, &tenant, id, false)
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
		for id in &settings.external_services {
			let (sql, values) = Query::select()
				.column(Alias::new("version"))
				.from(Alias::new("registry"))
				.and_where(Expr::col("tenant").eq(Expr::value(access.identity.tenant.clone())))
				.and_where(Expr::col("id").eq(Expr::value(id.clone())))
				.and_where(Expr::col("kind").eq(Expr::value("external_service")))
				.build(PostgresQueryBuilder);
			let rows = access
				.tx
				.fetch_all(&sql, convert_values(values))
				.await
				.map_err(crate::Error::from)?;
			let mut versions = Vec::new();
			for row in rows {
				let version: String = row
					.get("version")
					.map_err(reinhardt::core::exception::Error::from)
					.map_err(crate::Error::from)?;
				if let Ok(parsed) = semver::Version::parse(&version) {
					versions.push((parsed, version));
				}
			}
			let version = versions
				.into_iter()
				.max_by(|a, b| a.0.cmp(&b.0))
				.map(|(_, version)| version)
				.ok_or(aidash_application::Error::Forbidden)?;
			let reference = aidash_domain::registry::EntityRef {
				id: id.clone(),
				version,
			};
			let entry =
				crate::authorization::catalog::entry(access, &reference, "registry.read").await?;
			crate::marketplace::check_pinned(access, &entry).await?;
			aidash_domain::egress::definition(&entry)?;
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
							.from(Alias::new("runs"))
							.and_where(Expr::col("tenant").eq(Expr::value(tenant.clone())))
							.and_where(Expr::col("workspace_id").eq(Expr::value(run.workspace_id)))
							.and_where(
								Expr::col("home_node").eq(Expr::value(run.home_node.clone())),
							)
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
			// UUIDv8 is domain-separated from user-created UUIDv4 spaces and stable
			// across retries. Existing spaces need no Node allocation lock.
			let scratch_id = remote_space_id(
				&tenant,
				&run.node,
				&run.home_node,
				run.workspace_id,
				&person,
			);
			if let Some(space) = load_with_lock(&mut **access.tx, &tenant, scratch_id, false)
				.await
				.map_err(aidash_application::Error::from)?
			{
				if space.node != run.node || space.creator != person {
					return Err(aidash_application::Error::Forbidden);
				}
				return Ok((scratch_id, person));
			}
			// Serialize first allocation without blocking foreign-key checks.
			let (sql, values) = Query::select()
				.column(Alias::new("name"))
				.from(Alias::new("nodes"))
				.and_where(Expr::col("tenant").eq(Expr::value(tenant.clone())))
				.and_where(Expr::col("name").eq(Expr::value(run.node.as_str().to_owned())))
				.lock(LockType::NoKeyUpdate)
				.build(PostgresQueryBuilder);
			access
				.tx
				.fetch_optional(&sql, convert_values(values))
				.await
				.map_err(crate::Error::from)?
				.ok_or(aidash_application::Error::Forbidden)?;
			let existing = load(&mut **access.tx, &tenant, scratch_id)
				.await
				.map_err(aidash_application::Error::from)?;
			if let Some(space) = existing {
				if space.node != run.node || space.creator != person {
					return Err(aidash_application::Error::Forbidden);
				}
			} else {
				let mut scope = ScratchAdapter {
					access,
					services: self,
					person_override: Some(person.clone()),
				};
				let (ceilings, tenant_hosts) =
					aidash_application::scratch::ScratchScope::limits(&mut scope, &run.node)
						.await?;

				let now = chrono::Utc::now();
				let space = ScratchSpace {
					id: scratch_id,
					node: run.node.clone(),
					name: "Delegated Run".into(),
					creator: person.clone(),
					members: Default::default(),
					state: ScratchState::Active,
					revision: 0,
					wipe_generation: 0,
					settings: ScratchSettings::for_policy(&ceilings, &tenant_hosts),
					created_at: now,
					last_used_at: now,
				};
				aidash_application::scratch::create_dedicated(&mut scope, &space).await?;
			}
			return Ok((scratch_id, person));
		}
		let (sql, values) = Query::select()
			.columns([Alias::new("scratch_space_id"), Alias::new("created_by")])
			.from(Alias::new("workspaces"))
			.and_where(Expr::col("id").eq(Expr::value(run.workspace_id)))
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
