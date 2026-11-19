//! The calls of remote Runs that wait for an answer about a Federation tool list (ADR 0139), and
//! the tools one side allowed for one Run only. Every statement runs in the Tenant's scope.
use super::remote_grants::{run_live, unexpired};
use crate::Result;
use aidash_domain::federation::tool_lists::{ToolAsk, ToolList, ToolPattern, receiver_may_answer};
use aidash_domain::nodes::NodeAddress;
use aidash_domain::run_state::PausedCall;
use aidash_domain::{ControlSource, RunControl};
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::types::PgBinOper;
use reinhardt::query::{
	Alias, BinOper, Expr, ExprTrait as _, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SelectStatement, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;

/// The most asks one registration lists.
const LIMIT: u64 = 100;

/// `column -> 'a' -> 'b' …`, or `->>` for the last key when `as_text`: the JSON value at `path`
/// in `column`, `NULL` where the path is missing, as `#>`/`#>>` read it.
fn json_path(column: &str, path: &[&str], as_text: bool) -> SimpleExpr {
	let last = path.len().saturating_sub(1);
	let column: SimpleExpr = Expr::col(Alias::new(column)).into();
	path.iter().enumerate().fold(column, |value, (index, key)| {
		let operator = if as_text && index == last {
			PgBinOper::JsonGetAsText
		} else {
			PgBinOper::JsonGetByIndex
		};
		value.binary(BinOper::PgOperator(operator), Expr::value(*key).into())
	})
}

/// The call a Run's Authority Pause stopped, `pending.recovery.paused_call`.
fn paused_call() -> SimpleExpr {
	json_path("pending", &["recovery", "paused_call"], false)
}

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

/// The Runs of `node` admitted from `source` that wait in the Authority Pause of a call the
/// receiving Node's lists lack, and nothing else holds: a person's pause or a Safety Stop laid
/// over it replaces the `system` control source the pause left
/// ([`receiver_may_answer`]).
fn receiver_asks_query(tenant: &str, node: &str, source: &NodeAddress) -> SelectStatement {
	let mut query = Query::select();
	query
		.column(Alias::new("id"))
		.column(Alias::new("updated_at"))
		.expr_as(paused_call(), Alias::new("paused_call"))
		.from(Alias::new("runs"))
		.and_where(Expr::col(Alias::new("control")).eq("PAUSED"))
		.and_where(Expr::col(Alias::new("control_source")).eq(ControlSource::System.as_str()))
		.and_where(
			json_path(
				"pending",
				&["recovery", "paused_call", "ask", "lacking", "receiver"],
				true,
			)
			.eq("true"),
		)
		.and_where(
			Expr::col(Alias::new("id")).in_subquery(
				Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("authorization_remote_admissions"))
					.and_where(Expr::col(Alias::new("tenant")).eq(tenant))
					.and_where(Expr::col(Alias::new("node")).eq(node))
					.and_where(Expr::col(Alias::new("source_node")).eq(source.to_string()))
					.and_where(Expr::col(Alias::new("cancelled")).not())
					.to_owned(),
			),
		)
		.order_by(Alias::new("updated_at"), Order::Asc)
		.limit(LIMIT);
	query.to_owned()
}

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

/// The Runs of `node` admitted from `source` that wait for the receiving Node's people on a
/// call its lists lack ([`receiver_asks_query`]).
pub(crate) async fn receiver_asks(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
	source: &NodeAddress,
) -> Result<Vec<ReceiverAsk>> {
	let query = receiver_asks_query(tenant, node, source);
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

/// Whether `run` is a remote Run of `node` admitted from `source`, whatever it waits for now.
pub(crate) async fn admitted_from(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
	source: &NodeAddress,
	run: Uuid,
) -> Result<bool> {
	let found: Option<Uuid> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("authorization_remote_admissions"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(run)))
			.and_where(Expr::col(Alias::new("tenant")).eq(tenant))
			.and_where(Expr::col(Alias::new("node")).eq(node))
			.and_where(Expr::col(Alias::new("source_node")).eq(source.to_string()))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut *tx)
	.await?;
	Ok(found.is_some())
}

struct PauseRow {
	control: String,
	control_source: String,
	paused_call: Option<Value>,
}
crate::native_record!(PauseRow {
	control,
	control_source,
	paused_call
});

/// Lock the Run `run` and say whether it still waits for the receiving Node's people on the
/// call `call` about `key` and on nothing else ([`receiver_may_answer`]). The lock holds until
/// the caller's transaction ends, so no pause, resume or other answer comes in between.
pub(crate) async fn lock_receiver_ask(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
	call: &str,
	key: &str,
) -> Result<bool> {
	let row: Option<PauseRow> = crate::database::native::query_as(
		&Query::select()
			.column(Alias::new("control"))
			.column(Alias::new("control_source"))
			.expr_as(paused_call(), Alias::new("paused_call"))
			.from(Alias::new("runs"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(run)))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut *tx)
	.await?;
	let Some(row) = row else {
		return Ok(false);
	};
	let control: RunControl = serde_json::from_value(Value::String(row.control))?;
	let source = ControlSource::try_from(row.control_source)?;
	let paused = row
		.paused_call
		.and_then(|value| serde_json::from_value::<PausedCall>(value).ok());
	Ok(receiver_may_answer(
		control,
		source,
		paused.as_ref(),
		call,
		key,
	))
}

pub(crate) struct HomeGrant {
	pub id: Uuid,
	pub task_id: Uuid,
}
crate::native_record!(HomeGrant { id, task_id });

/// The Remote Grants `node` issued to `receiver` that are live and whose Run lives, so it may
/// wait for the Home's people; the soonest to expire first, a fixed order for a bounded page.
fn home_grants_query(
	tenant: &str,
	node: &str,
	receiver: &NodeAddress,
	only: Option<Uuid>,
) -> SelectStatement {
	let mut query = Query::select();
	query
		.column(Alias::new("id"))
		.column(Alias::new("task_id"))
		.from(Alias::new("authorization_remote_grants"))
		.and_where(Expr::col(Alias::new("tenant")).eq(tenant))
		.and_where(Expr::col(Alias::new("node")).eq(node))
		.and_where(Expr::col(Alias::new("node_id")).eq(receiver.to_string()))
		.and_where(unexpired())
		.and_where(run_live())
		.order_by(Alias::new("expires_at"), Order::Asc)
		.order_by(Alias::new("id"), Order::Asc)
		.limit(LIMIT);
	if let Some(id) = only {
		query.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)));
	}
	query.to_owned()
}

/// [`home_grants_query`]. `only` narrows the answer to one grant.
pub(crate) async fn home_grants(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
	receiver: &NodeAddress,
	only: Option<Uuid>,
) -> Result<Vec<HomeGrant>> {
	let query = home_grants_query(tenant, node, receiver, only);
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

#[cfg(test)]
mod tests {
	use super::*;

	fn source() -> NodeAddress {
		NodeAddress::parse("aidash://calm-heron/able-oak-abcd1234").unwrap()
	}

	/// The rendered statement keeps the predicate the raw SQL had (`pending #> '{recovery,
	/// paused_call}'`, `control = 'PAUSED'`, `id IN (SELECT id FROM authorization_remote_admissions
	/// WHERE tenant = ? AND node = ? AND source_node = ? AND NOT cancelled)`) and adds the control
	/// source, so a Run another pause holds is not listed.
	#[test]
	fn receiver_asks_are_built_without_raw_sql() {
		let sql = receiver_asks_query("acme", "default", &source()).to_string(PostgresQueryBuilder);
		assert_eq!(
			sql,
			"SELECT \"id\", \"updated_at\", \"pending\" -> 'recovery' -> 'paused_call' AS \"paused_call\" \
			FROM \"runs\" WHERE \"control\" = 'PAUSED' AND \"control_source\" = 'system' \
			AND \"pending\" -> 'recovery' -> 'paused_call' -> 'ask' -> 'lacking' ->> 'receiver' = 'true' \
			AND \"id\" IN (SELECT \"id\" FROM \"authorization_remote_admissions\" \
			WHERE \"tenant\" = 'acme' AND \"node\" = 'default' \
			AND \"source_node\" = 'aidash://calm-heron/able-oak-abcd1234' AND NOT \"cancelled\") \
			ORDER BY \"updated_at\" ASC LIMIT 100"
		);
	}

	/// The raw `tenant = ? AND node = ? AND node_id = ? AND NOT revoked AND expires_at >
	/// CLOCK_TIMESTAMP()`, narrowed to grants whose Run lives and ordered by expiry.
	#[test]
	fn home_grants_are_built_without_raw_sql_and_only_of_live_runs() {
		let sql = home_grants_query("acme", "default", &source(), Some(Uuid::nil()))
			.to_string(PostgresQueryBuilder);
		assert_eq!(
			sql,
			"SELECT \"id\", \"task_id\" FROM \"authorization_remote_grants\" \
			WHERE \"tenant\" = 'acme' AND \"node\" = 'default' \
			AND \"node_id\" = 'aidash://calm-heron/able-oak-abcd1234' \
			AND NOT \"revoked\" AND \"expires_at\" > CLOCK_TIMESTAMP() \
			AND EXISTS (SELECT TRUE FROM \"authorization_remote_execution\" AS \"b\" \
			INNER JOIN \"tasks\" AS \"t\" ON \"t\".\"id\" = \"b\".\"task_id\" \
			AND \"t\".\"revision\" = \"b\".\"task_revision\" \
			WHERE \"b\".\"grant_id\" = \"authorization_remote_grants\".\"id\" \
			AND \"t\".\"status\" NOT IN ('OPEN', 'COMPLETED', 'FAILED', 'CANCELLED', 'ABANDONED')) \
			AND \"id\" = '00000000-0000-0000-0000-000000000000' \
			ORDER BY \"expires_at\" ASC, \"id\" ASC LIMIT 100"
		);
	}
}
