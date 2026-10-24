//! Node deletion persistence. Lifecycle callers hold `nodes:<tenant>` before
//! recording or restoring; row locks fence restore against the purge worker.
use crate::Result;
use crate::apps::identity::models::NodeDeletion;
use aidash_domain::nodes::deletion::{self, Actor};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;

impl NodeDeletion {
	pub(crate) fn deleted_by(&self) -> Result<Actor> {
		Ok(Actor::parse(&self.deleted_by_kind)?)
	}
	pub(crate) fn rule(&self) -> Result<deletion::Deletion> {
		Ok(deletion::Deletion {
			deleted_by: self.deleted_by()?,
			purge_after: self.purge_after,
			restored: self.restored_at.is_some(),
			purged: self.purged_at.is_some(),
		})
	}
}

/// The unresolved deletion, including a completed purge. Closing a deletion
/// records a restore; deleting again inserts a new row with a fresh identity.
pub(crate) async fn current_locked(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
) -> Result<Option<NodeDeletion>> {
	Ok(NodeDeletion::objects()
		.filter(NodeDeletion::field_tenant().eq(tenant.to_owned()))
		.filter(NodeDeletion::field_node().eq(node.to_owned()))
		.filter(NodeDeletion::field_restored_at().is_null())
		.select_for_update()
		.all_with_executor(tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop())
}

/// Current deletion state for one bounded management page, without worker locks.
pub(crate) async fn current_of(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	nodes: Vec<String>,
) -> Result<std::collections::BTreeMap<String, NodeDeletion>> {
	if nodes.is_empty() {
		return Ok(Default::default());
	}
	Ok(NodeDeletion::objects()
		.filter(NodeDeletion::field_tenant().eq(tenant.to_owned()))
		.filter(NodeDeletion::field_node().is_in(nodes))
		.filter(NodeDeletion::field_restored_at().is_null())
		.all_with_executor(tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.into_iter()
		.map(|row| (row.node.clone(), row))
		.collect())
}

pub(crate) async fn record(
	tx: &mut reinhardt::db::orm::AtomicTransaction,
	tenant: &str,
	node: &str,
	actor: &crate::apps::identity::models::AdministrationActor,
	grace_days: u32,
) -> Result<NodeDeletion> {
	let now = crate::apps::identity::models::database_time(tx).await?;
	let (kind, identity) = actor.kind_and_identity();
	let row = NodeDeletion::build()
		.id(uuid::Uuid::new_v4())
		.tenant(tenant.to_owned())
		.node(node.to_owned())
		.deleted_at(now)
		.deleted_by_kind(kind.to_owned())
		.deleted_by_identity(identity)
		.purge_after(deletion::purge_after(now, grace_days)?)
		.restored_at(None)
		.restored_by_kind(None)
		.restored_by_identity(None)
		.purged_at(None)
		.purge_step(None)
		.purge_failures(0)
		.purge_error(None)
		.purge_failed_at(None)
		.purge_lease(None)
		.purge_leased_until(None)
		.audit_erased_at(None)
		.finish();
	super::nodes::fence_old_admission_snapshots(tx, tenant, node).await?;
	NodeDeletion::objects().create_with_conn(tx, &row).await?;
	Ok(row)
}

/// Restore touches only lifecycle state. Current policy, credentials and
/// registrations stay as they are; cancelled work is never restarted here.
pub(crate) async fn restore(
	tx: &mut dyn TransactionExecutor,
	row: &NodeDeletion,
	actor: &crate::apps::identity::models::AdministrationActor,
) -> Result<()> {
	use crate::Error;
	use reinhardt::db::orm::execution::convert_values;
	use reinhardt::query::{
		Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
	};
	let now = crate::apps::identity::models::database_time(tx).await?;
	let (kind, identity) = actor.kind_and_identity();
	deletion::check_restore(&row.rule()?, Actor::parse(kind)?, now)?;
	if row.purge_step.is_some() || row.purge_leased_until.is_some_and(|until| until > now) {
		return Err(Error::Conflict("node_purge_started".into()));
	}
	let (sql, values) = Query::update()
		.table(Alias::new(NodeDeletion::table_name()))
		.value(Alias::new("restored_at"), Expr::value(now))
		.value(Alias::new("restored_by_kind"), Expr::value(kind))
		.value(Alias::new("restored_by_identity"), Expr::value(identity))
		.and_where(Expr::col("id").eq(Expr::value(row.id)))
		.and_where(Expr::col("restored_at").is_null())
		.and_where(Expr::col("purged_at").is_null())
		.and_where(Expr::col("purge_step").is_null())
		.and_where(Expr::col("purge_after").gt(Expr::value(now)))
		.and_where(
			Expr::col("purge_leased_until")
				.is_null()
				.or(Expr::col("purge_leased_until").lte(Expr::value(now))),
		)
		.build(PostgresQueryBuilder);
	if tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected
		!= 1
	{
		return Err(Error::Conflict("node_purge_started".into()));
	}
	Ok(())
}

/// Requests the fenced worker without changing the immutable grace deadline.
/// Repeated requests preserve an already advanced purge step.
pub(crate) async fn request_purge(
	tx: &mut dyn TransactionExecutor,
	mut row: NodeDeletion,
) -> Result<NodeDeletion> {
	use crate::Error;
	use reinhardt::db::orm::execution::convert_values;
	use reinhardt::query::{
		Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
	};
	if row.purged_at.is_some() {
		return Err(Error::Conflict("node_purged".into()));
	}
	if row.purge_step.is_none() {
		let step = deletion::PurgeStep::FIRST.as_str();
		let (sql, values) = Query::update()
			.table(Alias::new(NodeDeletion::table_name()))
			.value(Alias::new("purge_step"), Expr::value(step))
			.and_where(Expr::col("id").eq(Expr::value(row.id)))
			.and_where(Expr::col("restored_at").is_null())
			.and_where(Expr::col("purged_at").is_null())
			.and_where(Expr::col("purge_step").is_null())
			.build(PostgresQueryBuilder);
		if tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected
			!= 1
		{
			return Err(Error::Conflict("Node purge state changed".into()));
		}
		row.purge_step = Some(step.to_owned());
	}
	Ok(row)
}
