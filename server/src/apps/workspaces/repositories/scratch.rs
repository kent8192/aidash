//! Scratch Space persistence on a caller-owned Tenant transaction.
use std::collections::BTreeSet;

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
	Alias, Expr, ExprTrait, IntoValue, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::Value;
use uuid::Uuid;

use crate::{Error, Result, apps::identity::services::access::Access};

/// Locks the row before loading dependent settings and members. The Tenant is
/// always the caller's scoped Tenant, never supplied by an API request.
pub(crate) async fn load(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
) -> Result<Option<ScratchSpace>> {
	load_with_lock(tx, tenant, id, true).await
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
		query.lock(LockType::Update);
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
	let (sql, values) = Query::select()
		.column(Alias::new("subject"))
		.from(Alias::new("scratch_space_members"))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant.to_owned())))
		.and_where(Expr::col("space_id").eq(Expr::value(id)))
		.build(PostgresQueryBuilder);
	let members: BTreeSet<String> = tx
		.fetch_all(&sql, convert_values(values))
		.await?
		.iter()
		.map(|row| row.get("subject").map_err(err))
		.collect::<std::result::Result<_, _>>()?;
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
	let state = serde_json::to_value(space.state)?
		.as_str()
		.ok_or_else(|| Error::Invalid("invalid Scratch Space state".into()))?
		.to_owned();
	let (sql, values) = Query::insert()
		.into_table(Alias::new("scratch_spaces"))
		.columns(
			[
				"id",
				"tenant",
				"node",
				"name",
				"creator",
				"state",
				"created_at",
				"last_used_at",
			]
			.map(Alias::new),
		)
		.values_panic([
			IntoValue::into_value(space.id),
			IntoValue::into_value(tenant.to_owned()),
			IntoValue::into_value(space.node.as_str().to_owned()),
			IntoValue::into_value(space.name.clone()),
			IntoValue::into_value(space.creator.clone()),
			IntoValue::into_value(state),
			IntoValue::into_value(space.created_at),
			IntoValue::into_value(space.last_used_at),
		])
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	let (sql, values) = Query::insert()
		.into_table(Alias::new("scratch_space_settings"))
		.columns(["space_id", "tenant", "settings"].map(Alias::new))
		.values_panic([
			IntoValue::into_value(space.id),
			IntoValue::into_value(tenant.to_owned()),
			IntoValue::into_value(serde_json::to_value(&space.settings)?),
		])
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	write_members(tx, tenant, space).await
}

async fn write_members(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	space: &ScratchSpace,
) -> Result<()> {
	for subject in &space.members {
		let (sql, values) = Query::insert()
			.into_table(Alias::new("scratch_space_members"))
			.columns(["id", "tenant", "space_id", "subject"].map(Alias::new))
			.values_panic([
				IntoValue::into_value(Uuid::new_v4()),
				IntoValue::into_value(tenant.to_owned()),
				IntoValue::into_value(space.id),
				IntoValue::into_value(subject.clone()),
			])
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
	}
	Ok(())
}

/// Replace mutable fields while the caller holds the space row lock.
pub(crate) async fn save(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	space: &ScratchSpace,
) -> Result<()> {
	let state = serde_json::to_value(space.state)?
		.as_str()
		.ok_or_else(|| Error::Invalid("invalid Scratch Space state".into()))?
		.to_owned();
	let (sql, values) = Query::update()
		.table(Alias::new("scratch_spaces"))
		.value(Alias::new("state"), state)
		.value(Alias::new("revision"), space.revision)
		.value(Alias::new("last_used_at"), space.last_used_at)
		.and_where(Expr::col("tenant").eq(Expr::value(tenant.to_owned())))
		.and_where(Expr::col("id").eq(Expr::value(space.id)))
		.build(PostgresQueryBuilder);
	if tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected
		!= 1
	{
		return Err(Error::NotFound("Scratch Space".into()));
	}
	let (sql, values) = Query::update()
		.table(Alias::new("scratch_space_settings"))
		.value(
			Alias::new("settings"),
			serde_json::to_value(&space.settings)?,
		)
		.and_where(Expr::col("tenant").eq(Expr::value(tenant.to_owned())))
		.and_where(Expr::col("space_id").eq(Expr::value(space.id)))
		.build(PostgresQueryBuilder);
	if tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected
		!= 1
	{
		return Err(Error::NotFound("Scratch Space settings".into()));
	}
	let (sql, values) = Query::delete()
		.from_table(Alias::new("scratch_space_members"))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant.to_owned())))
		.and_where(Expr::col("space_id").eq(Expr::value(space.id)))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	write_members(tx, tenant, space).await
}

/// Return the IDs on one Node; each space is subsequently locked and checked.
pub(crate) async fn ids_on_node(
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

pub(crate) async fn delete(tx: &mut dyn TransactionExecutor, tenant: &str, id: Uuid) -> Result<()> {
	let (sql, values) = Query::delete()
		.from_table(Alias::new("scratch_spaces"))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant.to_owned())))
		.and_where(Expr::col("id").eq(Expr::value(id)))
		.build(PostgresQueryBuilder);
	if tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected
		!= 1
	{
		return Err(Error::NotFound("Scratch Space".into()));
	}
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

fn effective_policy(
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
	/// Spaces belong to enabled people. A delegated Run's space instead
	/// belongs to its Peer Mapping's credential subject, which is always an
	/// exportable service or agent credential; only that allocation sets
	/// `person_override`.
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
						SubjectKind::User => true,
						SubjectKind::Service | SubjectKind::Agent => delegated,
						SubjectKind::Node => false,
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
		let ids = ids_on_node(&mut **self.access.tx, &tenant, node.as_str())
			.await
			.map_err(aidash_application::Error::from)?;
		let mut spaces = Vec::with_capacity(ids.len());
		for id in ids {
			if let Some(space) = load_with_lock(&mut **self.access.tx, &tenant, id, false)
				.await
				.map_err(aidash_application::Error::from)?
			{
				spaces.push(space);
			}
		}
		Ok(spaces)
	}
	async fn save(&mut self, space: &ScratchSpace) -> aidash_application::Result<()> {
		let tenant = self.access.identity.tenant.clone();
		save(&mut **self.access.tx, &tenant, space)
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
	async fn subnode_creator_run(&mut self, _: Uuid) -> aidash_application::Result<Option<Run>> {
		// Subnodes are not in the develop schema. Replace this when #670 lands.
		Ok(None)
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
		if !run.is_local(&access.node_id) {
			// A receiver may only allocate for its own already-admitted Run.
			// A shared lock keeps the admission from changing meanwhile without
			// blocking activation, which shares it too; the Node lock below
			// serializes first allocation.
			let tenant = access.identity.tenant.clone();
			let (sql, values) = Query::select()
				.column(Alias::new("credential_id"))
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
			let person: String = row
				.get("subject")
				.map_err(reinhardt::core::exception::Error::from)
				.map_err(crate::Error::from)?;
			if !access.subjects.contains(&person) {
				return Err(aidash_application::Error::Forbidden);
			}
			// Serialize first allocation on the receiving Node, including different
			// Runs of the same home Workspace. UUIDv8 is domain-separated from
			// user-created UUIDv4 spaces and stable across retries.
			let (sql, values) = Query::select()
				.column(Alias::new("name"))
				.from(Alias::new("nodes"))
				.and_where(Expr::col("tenant").eq(Expr::value(tenant.clone())))
				.and_where(Expr::col("name").eq(Expr::value(run.node.as_str().to_owned())))
				.lock(LockType::Update)
				.build(PostgresQueryBuilder);
			access
				.tx
				.fetch_optional(&sql, convert_values(values))
				.await
				.map_err(crate::Error::from)?
				.ok_or(aidash_application::Error::Forbidden)?;
			let scratch_id = remote_space_id(
				&tenant,
				&run.node,
				&run.home_node,
				run.workspace_id,
				&person,
			);
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
				let (_, tenant_hosts) =
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
					settings: ScratchSettings::for_tenant(&tenant_hosts),
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
		let starter = row
			.get("created_by")
			.map_err(err)
			.map_err(crate::Error::from)?;
		Ok((id, starter))
	}
}

#[cfg(test)]
mod tests;
