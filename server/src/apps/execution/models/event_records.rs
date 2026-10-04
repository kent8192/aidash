//! Ordered event insertion in a caller-owned native transaction.
use super::Event;
use crate::Result;
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
	node: &str,
	workspace: Option<Uuid>,
	kind: &str,
	data: Value,
) -> Result<()> {
	insert(tx, node, workspace, kind, data).await?;
	Ok(())
}

pub(crate) async fn create(
	tx: &mut AtomicTransaction,
	node: &str,
	workspace: Option<Uuid>,
	kind: &str,
	data: Value,
) -> Result<Event> {
	let id = insert(tx, node, workspace, kind, data).await?;
	Ok(Event::objects()
		.filter(Event::field_id().eq(id))
		.get_with_db(tx)
		.await?)
}

async fn insert(
	tx: &mut dyn TransactionExecutor,
	node: &str,
	workspace: Option<Uuid>,
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
			Alias::new("node_id"),
			Alias::new("workspace_id"),
			Alias::new("kind"),
			Alias::new("data"),
		])
		.values_panic([
			IntoValue::into_value(id),
			IntoValue::into_value(node),
			IntoValue::into_value(workspace),
			IntoValue::into_value(kind),
			IntoValue::into_value(data),
		])
		.build(PostgresQueryBuilder);
	TransactionExecutor::execute(tx, &statement, convert_values(values)).await?;
	Ok(id)
}

use reinhardt::query::IntoValue;
