//! The calls of remote Runs that wait for an answer about a Federation tool list (ADR 0139), and
//! the tools one side allowed for one Run only. Every statement runs in the Tenant's scope.
use crate::Result;
use aidash_domain::federation::tool_lists::{ToolAsk, ToolList, ToolPattern};
use aidash_domain::nodes::NodeAddress;
use aidash_domain::run_state::PausedCall;
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;

/// The most asks one registration lists.
const LIMIT: u64 = 100;

struct PausedRow {
	id: Uuid,
	updated_at: DateTime<Utc>,
	paused_call: Option<Value>,
}
crate::native_record!(PausedRow {
	id,
	updated_at,
	paused_call
});

struct AdmissionRow {
	task_id: Uuid,
	grant_id: Uuid,
	description: Value,
}
crate::native_record!(AdmissionRow {
	task_id,
	grant_id,
	description
});

/// A remote Run on this Node, admitted from the Home, that waits for this Node's people.
pub(crate) struct ReceiverAsk {
	/// The admission, which is also the Run.
	pub run: Uuid,
	/// The Home's grant.
	pub grant_id: Uuid,
	pub task_id: Uuid,
	pub task_title: Option<String>,
	pub paused: PausedCall,
	pub ask: ToolAsk,
	pub paused_since: DateTime<Utc>,
}

/// The Runs of `node` admitted from `source` that are paused on a call the receiving Node's
/// Allowance lacks. `only` narrows the answer to one Run.
pub(crate) async fn receiver_asks(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
	source: &NodeAddress,
	only: Option<Uuid>,
) -> Result<Vec<ReceiverAsk>> {
	let mut query = Query::select();
	query
		.column(Alias::new("id"))
		.column(Alias::new("updated_at"))
		.expr(Expr::cust("pending #> '{recovery,paused_call}' AS paused_call"))
		.from(Alias::new("runs"))
		.and_where(SimpleExpr::CustomWithExpr(
			"(control = 'PAUSED' AND (pending #>> '{recovery,paused_call,ask,lacking,receiver}') = 'true' AND id IN (SELECT id FROM authorization_remote_admissions WHERE tenant = ? AND node = ? AND source_node = ? AND NOT cancelled))"
				.to_owned(),
			vec![
				Expr::value(tenant.to_owned()).into(),
				Expr::value(node.to_owned()).into(),
				Expr::value(source.to_string()).into(),
			],
		))
		.order_by(Alias::new("updated_at"), Order::Asc)
		.limit(LIMIT);
	if let Some(id) = only {
		query.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)));
	}
	let rows: Vec<PausedRow> =
		crate::database::native::query_as(&query.to_string(PostgresQueryBuilder))
			.fetch_all(&mut *tx)
			.await?;
	let mut asks = Vec::new();
	for row in rows {
		let Some(paused) = row
			.paused_call
			.and_then(|value| serde_json::from_value::<PausedCall>(value).ok())
		else {
			continue;
		};
		let Some(ask) = paused.ask.clone().filter(|ask| ask.lacking.receiver) else {
			continue;
		};
		let admission: Option<AdmissionRow> = crate::database::native::query_as(
			&Query::select()
				.column(Alias::new("task_id"))
				.column(Alias::new("grant_id"))
				.column(Alias::new("description"))
				.from(Alias::new("authorization_remote_admissions"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(row.id)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut *tx)
		.await?;
		let Some(admission) = admission else {
			continue;
		};
		asks.push(ReceiverAsk {
			run: row.id,
			grant_id: admission.grant_id,
			task_id: admission.task_id,
			// The Description pins the task as the Home described it.
			task_title: admission
				.description
				.pointer("/task/title")
				.and_then(Value::as_str)
				.map(str::to_owned),
			paused,
			ask,
			paused_since: row.updated_at,
		});
	}
	Ok(asks)
}

pub(crate) struct HomeGrant {
	pub id: Uuid,
	pub task_id: Uuid,
}
crate::native_record!(HomeGrant { id, task_id });

/// The live Remote Grants `node` issued to `receiver`, whose Runs may wait for the Home's people.
/// `only` narrows the answer to one grant.
pub(crate) async fn home_grants(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
	receiver: &NodeAddress,
	only: Option<Uuid>,
) -> Result<Vec<HomeGrant>> {
	let mut query = Query::select();
	query
		.column(Alias::new("id"))
		.column(Alias::new("task_id"))
		.from(Alias::new("authorization_remote_grants"))
		.and_where(SimpleExpr::CustomWithExpr(
			"(tenant = ? AND node = ? AND node_id = ? AND NOT revoked AND expires_at > CLOCK_TIMESTAMP())"
				.to_owned(),
			vec![
				Expr::value(tenant.to_owned()).into(),
				Expr::value(node.to_owned()).into(),
				Expr::value(receiver.to_string()).into(),
			],
		))
		.order_by(Alias::new("id"), Order::Asc)
		.limit(LIMIT);
	if let Some(id) = only {
		query.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)));
	}
	crate::database::native::query_as(&query.to_string(PostgresQueryBuilder))
		.fetch_all(&mut *tx)
		.await
}

/// A Task's title, for a person who answers an ask about its Run.
pub(crate) async fn task_title(
	tx: &mut dyn TransactionExecutor,
	task: Uuid,
) -> Result<Option<String>> {
	let title: Option<String> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("title"))
			.from(Alias::new("tasks"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(task)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut *tx)
	.await?;
	Ok(title)
}

/// Add `pattern` to the Run-only list kept in `column` of `table` for `id`, under a row lock, and
/// answer the whole list.
async fn add_to_run_list(
	tx: &mut dyn TransactionExecutor,
	(table, column): (&str, &str),
	id: Uuid,
	pattern: ToolPattern,
) -> Result<ToolList> {
	let (sql, values) = Query::select()
		.column(Alias::new(column))
		.from(Alias::new(table))
		.and_where(Expr::col("id").eq(Expr::value(id)))
		.lock(LockType::Update)
		.build(PostgresQueryBuilder);
	let row = tx
		.fetch_optional(&sql, convert_values(values))
		.await?
		.ok_or_else(|| crate::Error::NotFound("remote execution not found".into()))?;
	let current: ToolList =
		super::node_registrations::optional_json(&row, column)?.unwrap_or_default();
	let updated = current.with(pattern)?;
	let (sql, values) = Query::update()
		.table(Alias::new(table))
		.value(Alias::new(column), serde_json::to_value(&updated)?)
		.and_where(Expr::col("id").eq(Expr::value(id)))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(updated)
}

/// The receiving Node's people allowed `pattern` for the Run `run` only.
pub(crate) async fn allow_run_only_on_receiver(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
	pattern: ToolPattern,
) -> Result<ToolList> {
	add_to_run_list(
		tx,
		("authorization_remote_admissions", "run_tool_allowance"),
		run,
		pattern,
	)
	.await
}

/// The Home's people allowed `pattern` for the grant `grant`'s Run only.
pub(crate) async fn allow_run_only_on_home(
	tx: &mut dyn TransactionExecutor,
	grant: Uuid,
	pattern: ToolPattern,
) -> Result<ToolList> {
	add_to_run_list(
		tx,
		("authorization_remote_grants", "tool_request_additions"),
		grant,
		pattern,
	)
	.await
}
