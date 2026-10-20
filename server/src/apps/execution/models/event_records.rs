//! Ordered event insertion in a caller-owned native transaction.
use super::Event;
use crate::database::scope::workspace_node;
use crate::{Result, database::scope::event_tenant};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{AtomicTransaction, Model};
use reinhardt::query::{
	Alias, Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;

pub(crate) async fn append(
	tx: &mut dyn TransactionExecutor,
	workspace: Option<Uuid>,
	kind: &str,
	data: Value,
) -> Result<()> {
	insert(tx, workspace, event_tenant(workspace), kind, data).await?;
	Ok(())
}

/// An event with no Workspace that belongs to `tenant`, for a transaction whose
/// Deployment Scope names no Tenant of its own.
pub(crate) async fn append_for_tenant(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	kind: &str,
	data: Value,
) -> Result<()> {
	insert(tx, None, Expr::value(tenant.to_owned()).into(), kind, data).await?;
	Ok(())
}

/// An event with no Workspace that concerns the Run `run`, such as the end of a
/// Run whose Home is another Node: it belongs to the Run's Tenant.
pub(crate) async fn append_for_run(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
	kind: &str,
	data: Value,
) -> Result<()> {
	let tenant = SimpleExpr::CustomWithExpr(
		"(SELECT tenant FROM runs WHERE id = ?)".to_owned(),
		vec![Expr::value(run).into()],
	);
	insert(tx, None, tenant, kind, data).await?;
	Ok(())
}

pub(crate) async fn create(
	tx: &mut AtomicTransaction,
	workspace: Option<Uuid>,
	kind: &str,
	data: Value,
) -> Result<Event> {
	let id = insert(tx, workspace, event_tenant(workspace), kind, data).await?;
	Ok(Event::objects()
		.filter(Event::field_id().eq(id))
		.get_with_db(tx)
		.await?)
}

/// The Node of the Workspace an event concerns; a Tenant-level event has none.
pub(crate) fn event_node(workspace: Option<Uuid>) -> SimpleExpr {
	workspace.map_or_else(|| Expr::cust("NULL::text").into(), workspace_node)
}

async fn insert(
	tx: &mut dyn TransactionExecutor,
	workspace: Option<Uuid>,
	tenant: SimpleExpr,
	kind: &str,
	data: Value,
) -> Result<Uuid> {
	// Last-Event-ID relies on sequence allocation following commit order.
	let (lock, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![Expr::value(71003201_i64).into()],
		))
		.build(PostgresQueryBuilder);
	TransactionExecutor::execute(tx, &lock, convert_values(values)).await?;
	// PostgreSQL owns the event sequence; omit its generated primary key.
	let id = Uuid::new_v4();
	let (statement, values) = Query::insert()
		.into_table(Alias::new("events"))
		.columns([
			Alias::new("id"),
			Alias::new("node"),
			Alias::new("workspace_id"),
			Alias::new("kind"),
			Alias::new("data"),
			Alias::new("tenant"),
		])
		.values_expr(vec![
			IntoValue::into_value(id).into(),
			event_node(workspace),
			IntoValue::into_value(workspace).into(),
			IntoValue::into_value(kind).into(),
			IntoValue::into_value(data).into(),
			tenant,
		])
		.expect("one value per column")
		.build(PostgresQueryBuilder);
	TransactionExecutor::execute(tx, &statement, convert_values(values)).await?;
	Ok(id)
}

use reinhardt::query::IntoValue;
