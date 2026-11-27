//! The calls of remote Runs that wait for an answer about a Federation tool list (ADR 0139), and
//! the tools one side allowed for one Run only. Every statement runs in the Tenant's scope.
use super::remote_grants::{live_condition, run_holds_task};
use crate::Result;
use aidash_domain::federation::tool_lists::{AskSide, ToolAsk, ToolList, ToolPattern};
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

/// Bound per-page Run hydration to the same 25 candidates as the Home scan.
const RECEIVER_PAGE: usize = 25;
pub(crate) type AskKey = (DateTime<Utc>, Uuid);

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
/// ([`aidash_domain::federation::tool_lists::may_answer`]).
fn receiver_asks_query(
	tenant: &str,
	node: &str,
	source: &NodeAddress,
	before: Option<AskKey>,
) -> SelectStatement {
	let mut query = Query::select();
	query
		.column(Alias::new("id"))
		.column(Alias::new("updated_at"))
		.expr_as(paused_call(), Alias::new("paused_call"))
		.from(Alias::new("task_executions"))
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
		.order_by(Alias::new("updated_at"), Order::Desc)
		.order_by(Alias::new("id"), Order::Desc)
		.limit(RECEIVER_PAGE as u64 + 1);
	if let Some((updated_at, id)) = before {
		query.and_where(
			Expr::col(Alias::new("updated_at"))
				.lt(Expr::value(updated_at))
				.or(Expr::col(Alias::new("updated_at"))
					.eq(Expr::value(updated_at))
					.and(Expr::col(Alias::new("id")).lt(Expr::value(id)))),
		);
	}
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
	before: Option<AskKey>,
) -> Result<(Vec<ReceiverAsk>, Option<AskKey>)> {
	let query = receiver_asks_query(tenant, node, source, before);
	let mut rows: Vec<PausedRow> =
		crate::database::native::query_as(&query.to_string(PostgresQueryBuilder))
			.fetch_all(&mut *tx)
			.await?;
	let more = rows.len() > RECEIVER_PAGE;
	rows.truncate(RECEIVER_PAGE);
	let next = rows
		.last()
		.filter(|_| more)
		.map(|row| (row.updated_at, row.id));
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
	Ok((asks, next))
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

/// Lock the Run and say whether it still waits for the specified side on the
/// call about `key` and on nothing else. The lock holds until
/// the caller's transaction ends, so no pause, resume or other answer comes in between.
pub(crate) async fn lock_ask(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
	call: &str,
	key: &str,
	side: AskSide,
) -> Result<bool> {
	let row: Option<PauseRow> = crate::database::native::query_as(
		&Query::select()
			.column(Alias::new("control"))
			.column(Alias::new("control_source"))
			.expr_as(paused_call(), Alias::new("paused_call"))
			.from(Alias::new("task_executions"))
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
	Ok(aidash_domain::federation::tool_lists::may_answer(
		control,
		source,
		paused.as_ref(),
		call,
		key,
		side,
	))
}

pub(crate) struct HomeGrant {
	pub id: Uuid,
	expires_at: DateTime<Utc>,
	pub task_id: Uuid,
	pub task_title: Option<String>,
}
crate::native_record!(HomeGrant {
	id,
	expires_at,
	task_id,
	task_title
});

/// The Remote Grants `node` issued to `receiver` that are live and whose Run lives, so it may
/// wait for the Home's people, soonest to expire first.
fn home_grants_query(
	tenant: &str,
	node: &str,
	receiver: &NodeAddress,
	only: Option<Uuid>,
) -> SelectStatement {
	let mut query = Query::select();
	query
		.column(Alias::new("id"))
		.column(Alias::new("expires_at"))
		.column(Alias::new("task_id"))
		.expr_as(
			Expr::subquery(
				Query::select()
					.column(Alias::new("title"))
					.from(Alias::new("tasks"))
					.and_where(Expr::col((Alias::new("tasks"), Alias::new("id"))).equals((
						Alias::new("authorization_remote_grants"),
						Alias::new("task_id"),
					)))
					.to_owned(),
			),
			Alias::new("task_title"),
		)
		.from(Alias::new("authorization_remote_grants"))
		.and_where(Expr::col(Alias::new("tenant")).eq(tenant))
		.and_where(Expr::col(Alias::new("node")).eq(node))
		.and_where(Expr::col(Alias::new("node_id")).eq(receiver.to_string()))
		.and_where(live_condition("authorization_remote_grants"))
		.and_where(run_holds_task())
		.order_by(Alias::new("expires_at"), Order::Asc)
		.order_by(Alias::new("id"), Order::Asc);
	if let Some(id) = only {
		query.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)));
	}
	query.to_owned()
}

/// Look up the live grant targeted by a Home answer.
pub(crate) async fn home_grant(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
	receiver: &NodeAddress,
	id: Uuid,
) -> Result<Option<HomeGrant>> {
	let query = home_grants_query(tenant, node, receiver, Some(id));
	crate::database::native::query_as(&query.to_string(PostgresQueryBuilder))
		.fetch_optional(&mut *tx)
		.await
}

/// Bound the query and retain one lookahead row to tell whether the Home scan can continue.
pub(crate) async fn home_grant_page(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
	receiver: &NodeAddress,
	after: Option<(DateTime<Utc>, Uuid)>,
	limit: usize,
) -> Result<(Vec<HomeGrant>, Option<(DateTime<Utc>, Uuid)>)> {
	let mut query = home_grants_query(tenant, node, receiver, None);
	if let Some((expires_at, id)) = after {
		query.and_where(
			Expr::col(Alias::new("expires_at"))
				.gt(Expr::value(expires_at))
				.or(Expr::col(Alias::new("expires_at"))
					.eq(Expr::value(expires_at))
					.and(Expr::col(Alias::new("id")).gt(Expr::value(id)))),
		);
	}
	query.limit(limit as u64 + 1);
	let mut grants: Vec<HomeGrant> =
		crate::database::native::query_as(&query.to_string(PostgresQueryBuilder))
			.fetch_all(&mut *tx)
			.await?;
	let next = if grants.len() > limit {
		grants.truncate(limit);
		grants.last().map(|grant| (grant.expires_at, grant.id))
	} else {
		None
	};
	Ok((grants, next))
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
		let sql =
			receiver_asks_query("acme", "default", &source(), None).to_string(PostgresQueryBuilder);
		assert_eq!(
			sql,
			"SELECT \"id\", \"updated_at\", \"pending\" -> 'recovery' -> 'paused_call' AS \"paused_call\" \
			FROM \"task_executions\" WHERE \"control\" = 'PAUSED' AND \"control_source\" = 'system' \
			AND \"pending\" -> 'recovery' -> 'paused_call' -> 'ask' -> 'lacking' ->> 'receiver' = 'true' \
			AND \"id\" IN (SELECT \"id\" FROM \"authorization_remote_admissions\" \
			WHERE \"tenant\" = 'acme' AND \"node\" = 'default' \
			AND \"source_node\" = 'aidash://calm-heron/able-oak-abcd1234' AND NOT \"cancelled\") \
			ORDER BY \"updated_at\" DESC, \"id\" DESC LIMIT 26"
		);
	}

	/// `tenant = ? AND node = ? AND node_id = ?`, the Home's one liveness rule
	/// (`live_condition`) instead of the lifetime alone. An activated Run
	/// must already hold its Task to be considered for the Tool Ask page.
	#[test]
	fn home_grants_are_built_without_raw_sql_and_only_of_live_runs() {
		let sql = home_grants_query("acme", "default", &source(), Some(Uuid::nil()))
			.to_string(PostgresQueryBuilder);
		assert_eq!(
			sql,
			"SELECT \"id\", \"expires_at\", \"task_id\", (SELECT \"title\" FROM \"tasks\" WHERE \"tasks\".\"id\" = \"authorization_remote_grants\".\"task_id\") AS \"task_title\" \
			FROM \"authorization_remote_grants\" \
			WHERE \"tenant\" = 'acme' AND \"node\" = 'default' \
			AND \"node_id\" = 'aidash://calm-heron/able-oak-abcd1234' \
			AND NOT \"authorization_remote_grants\".\"revoked\" \
			AND (\"authorization_remote_grants\".\"expires_at\" > CLOCK_TIMESTAMP() \
			OR EXISTS (SELECT TRUE FROM \"authorization_remote_execution\" AS \"live_binding\" \
			INNER JOIN \"tasks\" AS \"live_task\" ON \"live_task\".\"id\" = \"live_binding\".\"task_id\" \
			AND \"live_task\".\"tenant\" = \"live_binding\".\"tenant\" \
			WHERE \"live_binding\".\"grant_id\" = \"authorization_remote_grants\".\"id\" \
			AND \"live_binding\".\"task_id\" = \"authorization_remote_grants\".\"task_id\" \
			AND \"live_binding\".\"activated_at\" IS NOT NULL \
			AND (array_length(\"authorization_remote_grants\".\"subject_chain\", 1) \
			= ANY(array_positions(\"authorization_remote_grants\".\"subject_chain\", \"live_task\".\"owner\")) \
			AND \"live_task\".\"status\" NOT IN ('COMPLETED', 'FAILED', 'CANCELLED', 'ABANDONED') \
			OR \"live_task\".\"status\" = 'OPEN' AND \"live_task\".\"owner\" IS NULL \
			AND \"live_task\".\"revision\" = \"authorization_remote_grants\".\"task_revision\" \
			AND \"live_task\".\"revision\" = \"live_binding\".\"task_revision\"))) \
			AND EXISTS (SELECT TRUE FROM \"authorization_remote_execution\" AS \"b\" \
			INNER JOIN \"tasks\" AS \"t\" ON \"t\".\"id\" = \"b\".\"task_id\" \
			AND \"t\".\"revision\" = \"b\".\"task_revision\" \
			WHERE \"b\".\"grant_id\" = \"authorization_remote_grants\".\"id\" \
			AND \"b\".\"activated_at\" IS NOT NULL \
			AND array_length(\"authorization_remote_grants\".\"subject_chain\", 1) \
			= ANY(array_positions(\"authorization_remote_grants\".\"subject_chain\", \"t\".\"owner\")) \
			AND \"t\".\"status\" NOT IN ('COMPLETED', 'FAILED', 'CANCELLED', 'ABANDONED') \
			AND \"t\".\"status\" <> 'OPEN') \
			AND \"id\" = '00000000-0000-0000-0000-000000000000' \
			ORDER BY \"expires_at\" ASC, \"id\" ASC"
		);
	}
}
