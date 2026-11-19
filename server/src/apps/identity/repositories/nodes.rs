//! Creating and reading a Tenant's Nodes (#241). Every Tenant gets a default
//! Node at creation; Tenant Administrators add more within "Nodes per Tenant".
use super::tenants::Creator;
use crate::apps::identity::models::{Node as NodeRow, NodeNameRoot, database_time};
use crate::{Error, Result};
use aidash_domain::nodes::{self, Node, NodeName};
use aidash_domain::tenancy::TenantName;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, IntoValue, LockType, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr,
};
use uuid::Uuid;

/// The default Node of a new Tenant, created right after its root record.
pub(crate) async fn create_default(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	creator: Creator,
) -> Result<Node> {
	create(tx, tenant, nodes::DEFAULT_LABEL, true, creator).await
}

/// Who creates a Node: whoever creates its Tenant or acts as a Tenant
/// Administrator, or an Agent's chain acting as its root person (#1012).
#[derive(Debug, Clone)]
pub(crate) enum NodeCreator {
	Tenant(Creator),
	Agent(AgentCreation),
}

/// The chain that created a Node, kept in the database-only creator columns
/// of identity 0109 (`created_by_subject`, `created_by_run_id`,
/// `created_by_task_id` and `creation_key`).
#[derive(Debug, Clone)]
pub(crate) struct AgentCreation {
	/// The person at the root of the chain.
	pub subject: String,
	pub run: Uuid,
	pub task: Uuid,
	/// The tool call's invocation key: a retried call finds its Node by it.
	pub key: String,
}

impl From<Creator> for NodeCreator {
	fn from(creator: Creator) -> Self {
		Self::Tenant(creator)
	}
}

impl NodeCreator {
	fn kind(&self) -> &'static str {
		match self {
			Self::Tenant(creator) => creator.kind(),
			Self::Agent(_) => "agent",
		}
	}
	fn identity(&self) -> Option<Uuid> {
		match self {
			Self::Tenant(creator) => creator.identity(),
			Self::Agent(_) => None,
		}
	}
}

/// Inserts a Node under a freshly drawn name. A collision draws again; a name
/// taken once is never reused, because `node_names` rows are never deleted.
pub(crate) async fn create(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	label: &str,
	is_default: bool,
	creator: impl Into<NodeCreator>,
) -> Result<Node> {
	let creator = creator.into();
	let now = database_time(tx).await?;
	for _ in 0..8 {
		let name = NodeName::from_entropy(*Uuid::new_v4().as_bytes()).into_string();
		let (sql, values) = Query::insert()
			.into_table(Alias::new(NodeNameRoot::table_name()))
			.columns(["name", "tenant", "created_at"].map(Alias::new))
			.values_panic([
				IntoValue::into_value(name.clone()),
				IntoValue::into_value(tenant.to_owned()),
				IntoValue::into_value(now),
			])
			.on_conflict(
				OnConflict::column(Alias::new("name"))
					.do_nothing()
					.to_owned(),
			)
			.build(PostgresQueryBuilder);
		if tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected
			!= 1
		{
			continue;
		}
		let mut columns = vec![
			"tenant",
			"name",
			"label",
			"is_default",
			"created_at",
			"created_by_kind",
			"created_by_identity",
		];
		let mut row = vec![
			IntoValue::into_value(tenant.to_owned()),
			IntoValue::into_value(name.clone()),
			IntoValue::into_value(label.to_owned()),
			IntoValue::into_value(is_default),
			IntoValue::into_value(now),
			IntoValue::into_value(creator.kind()),
			IntoValue::into_value(creator.identity()),
		];
		if let NodeCreator::Agent(agent) = &creator {
			columns.extend([
				"created_by_subject",
				"created_by_run_id",
				"created_by_task_id",
				"creation_key",
			]);
			row.extend([
				IntoValue::into_value(agent.subject.clone()),
				IntoValue::into_value(agent.run),
				IntoValue::into_value(agent.task),
				IntoValue::into_value(agent.key.clone()),
			]);
		}
		let (sql, values) = Query::insert()
			.into_table(Alias::new(NodeRow::table_name()))
			.columns(columns.into_iter().map(Alias::new))
			.values_panic(row)
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		return Ok(Node {
			tenant: TenantName::new(tenant)?,
			name: NodeName::parse(&name)?,
			label: label.to_owned(),
			default: is_default,
			created_at: now,
		});
	}
	Err(Error::Conflict("no free Node name was drawn".into()))
}

/// Serializes Node creation per Tenant, so concurrent requests at the limit
/// cannot all pass the count.
pub(crate) async fn lock_tenant(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<()> {
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![SimpleExpr::FunctionCall(
				"hashtextextended".into_iden(),
				vec![
					Expr::value(format!("nodes:{tenant}")).into(),
					Expr::value(24_100_241_i64).into(),
				],
			)],
		))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

const COLUMNS: [&str; 5] = ["tenant", "name", "label", "is_default", "created_at"];

fn node_of(row: &reinhardt::db::backends::Row) -> Result<Node> {
	let failed = reinhardt::core::exception::Error::from;
	let tenant: String = row.get("tenant").map_err(failed)?;
	let name: String = row.get("name").map_err(failed)?;
	Ok(Node {
		tenant: TenantName::new(tenant)?,
		name: NodeName::parse(&name)?,
		label: row.get("label").map_err(failed)?,
		default: row.get("is_default").map_err(failed)?,
		created_at: row.get("created_at").map_err(failed)?,
	})
}

async fn one(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	condition: reinhardt::query::SimpleExpr,
) -> Result<Option<Node>> {
	let (sql, values) = Query::select()
		.columns(COLUMNS.map(Alias::new))
		.from(Alias::new(NodeRow::table_name()))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.and_where(condition)
		.build(PostgresQueryBuilder);
	tx.fetch_optional(&sql, convert_values(values))
		.await?
		.as_ref()
		.map(node_of)
		.transpose()
}

/// One Node of `tenant`, or 404.
pub(crate) async fn get(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	name: &str,
) -> Result<Node> {
	one(tx, tenant, Expr::col("name").eq(Expr::value(name)))
		.await?
		.ok_or_else(|| Error::NotFound("node not found".into()))
}

/// The Tenant's default Node.
pub(crate) async fn default_of(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<Node> {
	one(tx, tenant, Expr::col("is_default").eq(Expr::value(true)))
		.await?
		.ok_or_else(|| Error::Conflict("the Tenant has no default Node".into()))
}

pub(crate) async fn relabel(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	name: &str,
	label: &str,
) -> Result<Node> {
	get_live(tx, tenant, name).await?;
	let (sql, values) = Query::update()
		.table(Alias::new(NodeRow::table_name()))
		.value(Alias::new("label"), Expr::value(label))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.and_where(Expr::col("name").eq(Expr::value(name)))
		.build(PostgresQueryBuilder);
	if tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected
		!= 1
	{
		return Err(Error::NotFound("node not found".into()));
	}
	get(tx, tenant, name).await
}

/// A live Node has no unresolved deletion, including a purge that completed.
fn live_condition() -> SimpleExpr {
	let mut deleted = Query::select();
	deleted
		.expr(Expr::value(1_i32))
		.from(Alias::new("node_deletions"))
		.and_where(
			Expr::col((Alias::new("node_deletions"), Alias::new("tenant")))
				.eq(Expr::col((Alias::new("nodes"), Alias::new("tenant")))),
		)
		.and_where(
			Expr::col((Alias::new("node_deletions"), Alias::new("node")))
				.eq(Expr::col((Alias::new("nodes"), Alias::new("name")))),
		)
		.and_where(Expr::col("restored_at").is_null());
	Expr::exists(deleted).not()
}

pub(crate) async fn get_live(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	name: &str,
) -> Result<Node> {
	one(
		tx,
		tenant,
		Expr::col("name")
			.eq(Expr::value(name))
			.and(live_condition()),
	)
	.await?
	.ok_or_else(|| Error::NotFound("node not found".into()))
}

/// Fresh atomic admissions call this while holding the policy SHARE lease.
/// Node deletion advances this row's MVCC version under the exclusive policy
/// lease, so a SERIALIZABLE snapshot from before deletion cannot admit work.
pub(crate) async fn require_live_locked(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	name: &str,
) -> Result<()> {
	let (sql, values) = Query::select()
		.column(Alias::new("name"))
		.from(Alias::new(NodeRow::table_name()))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.and_where(Expr::col("name").eq(Expr::value(name)))
		.and_where(live_condition())
		.lock(LockType::Share)
		.build(PostgresQueryBuilder);
	tx.fetch_optional(&sql, convert_values(values))
		.await?
		.ok_or(Error::Forbidden)?;
	Ok(())
}

/// The deletion owner holds policy exclusive, then Node capacity, before this
/// fence. Keep every logical field unchanged; only the row's MVCC version must
/// advance to invalidate fresh admissions using an older SERIALIZABLE snapshot.
pub(crate) async fn fence_old_admission_snapshots(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	name: &str,
) -> Result<()> {
	let (sql, values) = Query::update()
		.table(Alias::new(NodeRow::table_name()))
		.value_expr(Alias::new("label"), Expr::col("label"))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.and_where(Expr::col("name").eq(Expr::value(name)))
		.build(PostgresQueryBuilder);
	if tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected
		!= 1
	{
		return Err(Error::NotFound("node not found".into()));
	}
	Ok(())
}

pub(crate) async fn list_live(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<Vec<Node>> {
	let (sql, values) = Query::select()
		.columns(COLUMNS.map(Alias::new))
		.from(Alias::new(NodeRow::table_name()))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.and_where(live_condition())
		.order_by(Alias::new("is_default"), reinhardt::query::Order::Desc)
		.order_by(Alias::new("created_at"), reinhardt::query::Order::Asc)
		.order_by(Alias::new("name"), reinhardt::query::Order::Asc)
		.build(PostgresQueryBuilder);
	tx.fetch_all(&sql, convert_values(values))
		.await?
		.iter()
		.map(node_of)
		.collect()
}

pub(crate) async fn count_live(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<u32> {
	let (sql, values) = Query::select()
		.expr_as(
			reinhardt::query::Func::count(Expr::col(Alias::new("name")).into()),
			Alias::new("total"),
		)
		.from(Alias::new(NodeRow::table_name()))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.and_where(live_condition())
		.build(PostgresQueryBuilder);
	let total: i64 = tx
		.fetch_one(&sql, convert_values(values))
		.await?
		.get("total")
		.map_err(reinhardt::core::exception::Error::from)?;
	Ok(u32::try_from(total).unwrap_or(u32::MAX))
}

/// A bounded management page includes retained deleted Nodes.
pub(crate) async fn list_management(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	limit: u64,
	offset: u64,
) -> Result<Vec<Node>> {
	let (sql, values) = Query::select()
		.columns(COLUMNS.map(Alias::new))
		.from(Alias::new(NodeRow::table_name()))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.order_by(Alias::new("is_default"), reinhardt::query::Order::Desc)
		.order_by(Alias::new("created_at"), reinhardt::query::Order::Asc)
		.order_by(Alias::new("name"), reinhardt::query::Order::Asc)
		.limit(limit)
		.offset(offset)
		.build(PostgresQueryBuilder);
	tx.fetch_all(&sql, convert_values(values))
		.await?
		.iter()
		.map(node_of)
		.collect()
}

/// The caller holds the Tenant capacity lock and its policy lock.
pub(crate) async fn set_default(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	name: &str,
) -> Result<Node> {
	get_live(tx, tenant, name).await?;
	for (condition, value) in [
		(Expr::col("is_default").eq(Expr::value(true)), false),
		(Expr::col("name").eq(Expr::value(name)), true),
	] {
		let (sql, values) = Query::update()
			.table(Alias::new(NodeRow::table_name()))
			.value(Alias::new("is_default"), Expr::value(value))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
			.and_where(condition)
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
	}
	get_live(tx, tenant, name).await
}

/// Current executing authority, including physical existence and unresolved deletion.
/// Callers that mutate under this predicate hold the Tenant policy lease, which
/// serializes with Node deletion and restoration.
pub(crate) fn live_node_condition(tenant: SimpleExpr, node: SimpleExpr) -> SimpleExpr {
	let mut deleted = Query::select();
	deleted
		.expr(Expr::value(1_i32))
		.from_as(Alias::new("node_deletions"), Alias::new("closed_node"))
		.and_where(
			Expr::col((Alias::new("closed_node"), Alias::new("tenant")))
				.eq(Expr::col((Alias::new("live_node"), Alias::new("tenant")))),
		)
		.and_where(
			Expr::col((Alias::new("closed_node"), Alias::new("node")))
				.eq(Expr::col((Alias::new("live_node"), Alias::new("name")))),
		)
		.and_where(Expr::col((Alias::new("closed_node"), Alias::new("restored_at"))).is_null());
	let mut live = Query::select();
	live.expr(Expr::value(1_i32))
		.from_as(Alias::new("nodes"), Alias::new("live_node"))
		.and_where(Expr::col((Alias::new("live_node"), Alias::new("tenant"))).eq(tenant))
		.and_where(Expr::col((Alias::new("live_node"), Alias::new("name"))).eq(node))
		.and_where(Expr::exists(deleted).not());
	Expr::exists(live).into()
}

/// A Thread's durable identity does not retain authority after its Node closes.
pub(crate) fn live_thread_condition(thread: SimpleExpr, tenant: SimpleExpr) -> SimpleExpr {
	let mut live = Query::select();
	live.expr(Expr::value(1_i32))
		.from_as(Alias::new("threads"), Alias::new("live_thread"))
		.and_where(Expr::col((Alias::new("live_thread"), Alias::new("id"))).eq(thread))
		.and_where(Expr::col((Alias::new("live_thread"), Alias::new("tenant"))).eq(tenant))
		// A deleted Thread is gone for every reader and writer (#1008).
		.and_where(Expr::col((Alias::new("live_thread"), Alias::new("deleted_at"))).is_null())
		.and_where(live_node_condition(
			Expr::col((Alias::new("live_thread"), Alias::new("tenant"))).into(),
			Expr::col((Alias::new("live_thread"), Alias::new("node"))).into(),
		));
	Expr::exists(live).into()
}

/// Absence is a refusal, while persistence failures remain errors.
pub(crate) async fn is_live(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	name: &str,
) -> Result<bool> {
	match get_live(tx, tenant, name).await {
		Ok(_) => Ok(true),
		Err(Error::NotFound(_)) => Ok(false),
		Err(error) => Err(error),
	}
}

/// A saved execution identity losing its Node is authority withdrawal, not an outage.
pub(crate) async fn require_live(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	name: &str,
) -> Result<()> {
	if !is_live(tx, tenant, name).await? {
		return Err(Error::Forbidden);
	}
	Ok(())
}

/// The Node an Agent's tool call with this key already created, deleted or
/// not, so a retried call creates no second one (#1012).
pub(crate) async fn created_by_call(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	run: Uuid,
	key: &str,
) -> Result<Option<Node>> {
	one(
		tx,
		tenant,
		Expr::col("created_by_run_id")
			.eq(Expr::value(run))
			.and(Expr::col("creation_key").eq(Expr::value(key))),
	)
	.await
}

/// Who created each of `names` (all of the Tenant's Nodes when `None`), from
/// the creator columns, including the database-only ones of identity 0109.
pub(crate) async fn creators(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	names: Option<&[String]>,
) -> Result<std::collections::BTreeMap<String, aidash_domain::nodes::agents::NodeCreator>> {
	let mut query = Query::select();
	query
		.columns(
			[
				"name",
				"created_by_kind",
				"created_by_identity",
				"created_by_subject",
				"created_by_task_id",
				"created_by_run_id",
			]
			.map(Alias::new),
		)
		.from(Alias::new(NodeRow::table_name()))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)));
	if let Some(names) = names {
		if names.is_empty() {
			return Ok(Default::default());
		}
		query.and_where(Expr::col("name").is_in(names.iter().cloned()));
	}
	let rows = crate::database::native::query(&query.to_string(PostgresQueryBuilder))
		.fetch_all(&mut *tx)
		.await?;
	let mut creators = std::collections::BTreeMap::new();
	for row in rows {
		let name: String = row.try_get("name")?;
		let kind: String = row.try_get("created_by_kind")?;
		let creator = aidash_domain::nodes::agents::NodeCreator::from_columns(
			&kind,
			row.try_get("created_by_identity")?,
			row.try_get("created_by_subject")?,
			row.try_get("created_by_task_id")?,
			row.try_get("created_by_run_id")?,
		)?;
		creators.insert(name, creator);
	}
	Ok(creators)
}

/// The live Nodes Runs of `tasks` created (#1012): a flow's stop lists them.
pub(crate) async fn created_by_tasks(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	tasks: &[Uuid],
) -> Result<Vec<Node>> {
	if tasks.is_empty() {
		return Ok(vec![]);
	}
	let (sql, values) = Query::select()
		.columns(COLUMNS.map(Alias::new))
		.from(Alias::new(NodeRow::table_name()))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.and_where(
			Expr::col("created_by_task_id").is_in(tasks.iter().map(|task| Expr::value(*task))),
		)
		.and_where(live_condition())
		.order_by(Alias::new("name"), reinhardt::query::Order::Asc)
		.build(PostgresQueryBuilder);
	tx.fetch_all(&sql, convert_values(values))
		.await?
		.iter()
		.map(node_of)
		.collect()
}
