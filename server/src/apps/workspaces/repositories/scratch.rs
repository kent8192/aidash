//! Scratch Space persistence on a caller-owned Tenant transaction.
use std::collections::BTreeSet;

use aidash_domain::{
	Run,
	nodes::NodeName,
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

use crate::{Error, Result};

/// Locks the row before loading dependent settings and members. The Tenant is
/// always the caller's scoped Tenant, never supplied by an API request.
pub(crate) async fn load(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
) -> Result<Option<ScratchSpace>> {
	let (sql, values) = Query::select()
		.columns(
			[
				"id",
				"node",
				"name",
				"creator",
				"state",
				"created_at",
				"last_used_at",
			]
			.map(Alias::new),
		)
		.from(Alias::new("scratch_spaces"))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant.to_owned())))
		.and_where(Expr::col("id").eq(Expr::value(id)))
		.lock(LockType::Update)
		.build(PostgresQueryBuilder);
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

#[cfg(test)]
mod tests;

/// The authorization, deployment settings, and runtime bindings supplied by
/// the server composition. Every method operates in the caller's transaction.
#[async_trait::async_trait]
pub(crate) trait ScratchServices: Send {
	async fn require_create(
		&mut self,
		tx: &mut dyn TransactionExecutor,
		node: &NodeName,
	) -> aidash_application::Result<()>;
	async fn require_node_use(
		&mut self,
		tx: &mut dyn TransactionExecutor,
		node: &NodeName,
		person: &str,
	) -> aidash_application::Result<()>;
	async fn limits(
		&mut self,
		tx: &mut dyn TransactionExecutor,
		node: &NodeName,
	) -> aidash_application::Result<(ScratchCeilings, HostAllowlist)>;
	async fn validate_references(
		&mut self,
		tx: &mut dyn TransactionExecutor,
		node: &NodeName,
		settings: &ScratchSettings,
	) -> aidash_application::Result<()>;
	async fn validate_members(
		&mut self,
		tx: &mut dyn TransactionExecutor,
		members: &BTreeSet<String>,
	) -> aidash_application::Result<()>;
	async fn clear_contents(
		&mut self,
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
	) -> aidash_application::Result<()>;
	async fn run_binding(
		&mut self,
		tx: &mut dyn TransactionExecutor,
		run: &Run,
	) -> aidash_application::Result<(Uuid, String)>;
}

/// A transaction-scoped adapter; callers supply the authenticated Tenant and person.
pub(crate) struct ScratchAdapter<'a> {
	pub tx: &'a mut dyn TransactionExecutor,
	pub tenant: &'a str,
	pub person: &'a str,
	pub services: &'a mut dyn ScratchServices,
}

#[async_trait::async_trait]
impl aidash_application::scratch::ScratchScope for ScratchAdapter<'_> {
	fn person(&self) -> &str {
		self.person
	}
	async fn require_create(&mut self, node: &NodeName) -> aidash_application::Result<()> {
		self.services.require_create(self.tx, node).await
	}
	async fn require_node_use(
		&mut self,
		node: &NodeName,
		person: &str,
	) -> aidash_application::Result<()> {
		self.services.require_node_use(self.tx, node, person).await
	}
	async fn limits(
		&mut self,
		node: &NodeName,
	) -> aidash_application::Result<(ScratchCeilings, HostAllowlist)> {
		self.services.limits(self.tx, node).await
	}
	async fn validate_references(
		&mut self,
		node: &NodeName,
		settings: &ScratchSettings,
	) -> aidash_application::Result<()> {
		self.services
			.validate_references(self.tx, node, settings)
			.await
	}
	async fn validate_members(
		&mut self,
		members: &BTreeSet<String>,
	) -> aidash_application::Result<()> {
		self.services.validate_members(self.tx, members).await
	}
	async fn insert(&mut self, space: &ScratchSpace) -> aidash_application::Result<()> {
		insert(self.tx, self.tenant, space)
			.await
			.map_err(Into::into)
	}
	async fn load_for_update(&mut self, id: Uuid) -> aidash_application::Result<ScratchSpace> {
		load(self.tx, self.tenant, id)
			.await
			.map_err(aidash_application::Error::from)?
			.ok_or_else(|| aidash_application::Error::NotFound("Scratch Space".into()))
	}
	async fn list(&mut self, node: &NodeName) -> aidash_application::Result<Vec<ScratchSpace>> {
		let ids = ids_on_node(self.tx, self.tenant, node.as_str())
			.await
			.map_err(aidash_application::Error::from)?;
		let mut spaces = Vec::with_capacity(ids.len());
		for id in ids {
			if let Some(space) = load(self.tx, self.tenant, id)
				.await
				.map_err(aidash_application::Error::from)?
			{
				spaces.push(space);
			}
		}
		Ok(spaces)
	}
	async fn save(&mut self, space: &ScratchSpace) -> aidash_application::Result<()> {
		save(self.tx, self.tenant, space).await.map_err(Into::into)
	}
	async fn delete(&mut self, id: Uuid) -> aidash_application::Result<()> {
		delete(self.tx, self.tenant, id).await.map_err(Into::into)
	}
	async fn clear_contents(&mut self, id: Uuid) -> aidash_application::Result<()> {
		self.services.clear_contents(self.tx, id).await
	}
	async fn subnode_creator_run(&mut self, _: Uuid) -> aidash_application::Result<Option<Run>> {
		// Subnodes are not in the develop schema. Replace this when #670 lands.
		Ok(None)
	}
	async fn run_binding(&mut self, run: &Run) -> aidash_application::Result<(Uuid, String)> {
		self.services.run_binding(self.tx, run).await
	}
}
