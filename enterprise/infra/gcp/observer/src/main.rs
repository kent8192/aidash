// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Infrastructure-only, read-only activity snapshot. No application writes or migrations.
use reinhardt_db::backends::{
	DatabaseConnection, IsolationLevel, QueryValue, Row, TransactionExecutor,
};
use reinhardt_query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait, Func, IntoIden, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr,
};
use serde_json::{Value, json};
use std::{
	collections::{BTreeMap, HashSet},
	time::Duration,
};

type Error = Box<dyn std::error::Error + Send + Sync>;
type Transaction = Box<dyn TransactionExecutor>;

/// The tenancy scope this observer reads in, as the transaction-local settings
/// that row-level policies read ("Tenancy scope settings" in
/// server/migrations/README.md). This workspace cannot depend on the server,
/// so it writes the server's string contract by hand: a read-only Deployment
/// Scope for the `activity-observer` reason.
const SCOPE: [(&str, &str); 4] = [
	("aidash.scope", "deployment"),
	("aidash.tenant", ""),
	("application_name", "deployment:activity-observer"),
	("transaction_read_only", "on"),
];

fn call(function: &'static str, arguments: Vec<SimpleExpr>) -> SimpleExpr {
	SimpleExpr::FunctionCall(function.into_iden(), arguments)
}

/// Opens the observation's snapshot in its Deployment Scope. The settings are
/// written before any other statement, so the snapshot starts with them.
#[allow(clippy::disallowed_methods)] // The scope is written before any other statement.
async fn begin(database: &DatabaseConnection) -> Result<Transaction, Error> {
	let mut tx = database
		.begin_with_isolation(IsolationLevel::RepeatableRead)
		.await?;
	let mut scope = Query::select();
	for (name, value) in SCOPE {
		scope.expr(call(
			"set_config",
			vec![
				Expr::value(name).into(),
				Expr::value(value).into(),
				Expr::value(true).into(),
			],
		));
	}
	tx.execute_generated(scope.build(PostgresQueryBuilder), None)
		.await?;
	Ok(tx)
}

fn column<'r>(row: &'r Row, name: &str) -> Result<&'r QueryValue, Error> {
	row.data
		.get(name)
		.ok_or_else(|| format!("missing column {name}").into())
}

fn integer(row: &Row, name: &str) -> Result<i64, Error> {
	match column(row, name)? {
		QueryValue::Int(value) => Ok(*value),
		_ => Err(format!("{name} is not an integer").into()),
	}
}

fn text(row: &Row, name: &str) -> Result<String, Error> {
	match column(row, name)? {
		QueryValue::String(value) => Ok(value.clone()),
		_ => Err(format!("{name} is not text").into()),
	}
}

fn number(row: &Row, name: &str) -> Result<Option<f64>, Error> {
	match column(row, name)? {
		QueryValue::Float(value) => Ok(Some(*value)),
		QueryValue::Int(value) => Ok(Some(*value as f64)),
		QueryValue::Null => Ok(None),
		_ => Err(format!("{name} is not a number").into()),
	}
}

fn document(row: &Row, name: &str) -> Result<Value, Error> {
	match column(row, name)? {
		QueryValue::Json(value) => Ok(value.as_deref().cloned().unwrap_or(Value::Null)),
		QueryValue::Null => Ok(Value::Null),
		_ => Err(format!("{name} is not JSON").into()),
	}
}

/// The human request or capability approval a Run's waiting state names, in the
/// `data` of its stored `pending` document. A Run waiting on its Home Node's
/// human request is not one: that request lives on another Node.
fn awaited_id(pending: &Value) -> Option<&str> {
	let wait = &pending["data"];
	match wait["reason"].as_str()? {
		"human" => wait["request_id"].as_str(),
		"approval" if wait["approval"]["source"] == "capability" => {
			wait["approval"]["approval_id"].as_str()
		}
		_ => None,
	}
}

fn human_wait(phase: &str, pending: &Value) -> bool {
	// A timed WAITING phase is work. A human/approval wait can also carry an
	// expiry wake_at; that does not extend the application's approval deadline.
	phase == "WAITING" && awaited_id(pending).is_some()
}

async fn count(tx: &mut Transaction, table: &str, condition: SimpleExpr) -> Result<i64, Error> {
	let query = Query::select()
		.expr_as(
			Func::count(Expr::col(ColumnRef::Asterisk).into()),
			Alias::new("total"),
		)
		.from(Alias::new(table))
		.and_where(condition)
		.build(PostgresQueryBuilder);
	integer(&tx.fetch_one_generated(query, None).await?, "total")
}

async fn waiting_ids(
	tx: &mut Transaction,
	table: &str,
	condition: SimpleExpr,
) -> Result<HashSet<String>, Error> {
	let query = Query::select()
		.expr_as(
			Expr::col(Alias::new("id")).cast_as("text"),
			Alias::new("id"),
		)
		.from(Alias::new(table))
		.and_where(condition)
		.build(PostgresQueryBuilder);
	let mut ids = HashSet::new();
	for row in tx.fetch_all_generated(query, None).await? {
		ids.insert(text(&row, "id")?);
	}
	Ok(ids)
}

/// Count and fingerprint the durable completed-transfer receipts in constant
/// memory, however many accumulate. Each receipt contributes PostgreSQL's native
/// 64-bit `uuid_hash_extended` of its ID; the contributions are summed (order-free,
/// so there is no sort and no buffered ID list) and the sum is hashed with SHA-256
/// into a fixed 64-character hex digest. Adding or removing a receipt changes the
/// digest; retries and object reclamation do not. The per-receipt work is one
/// native hash, so the summary costs no more than the other predicate scans of
/// `core_records` in this observation, instead of a SHA-256 per receipt.
async fn receipt_summary(tx: &mut Transaction) -> Result<(i64, String), Error> {
	let id_fold = call(
		"uuid_hash_extended",
		vec![
			Expr::col(Alias::new("id")).into(),
			Expr::value(0_i64).into(),
		],
	);
	let fold = Func::coalesce(vec![Func::sum(id_fold), Expr::value(0_i64).into()]).cast_as("text");
	let digest = call(
		"encode",
		vec![
			call(
				"sha256",
				vec![call("convert_to", vec![fold, Expr::value("UTF8").into()])],
			),
			Expr::value("hex").into(),
		],
	);
	let query = Query::select()
		.expr_as(
			Func::count(Expr::col(ColumnRef::Asterisk).into()),
			Alias::new("total"),
		)
		.expr_as(digest, Alias::new("digest"))
		.from(Alias::new("core_records"))
		.cond_where(
			Condition::any()
				.add(
					Expr::col(Alias::new("kind"))
						.eq("transfer_out")
						.and(Expr::col(Alias::new("state")).eq("delivered")),
				)
				.add(
					Expr::col(Alias::new("kind"))
						.eq("transfer_in")
						.and(Expr::col(Alias::new("state")).eq("committed")),
				),
		)
		.build(PostgresQueryBuilder);
	let row = tx.fetch_one_generated(query, None).await?;
	Ok((integer(&row, "total")?, text(&row, "digest")?))
}

#[allow(clippy::disallowed_methods)] // The observer has no server runtime to connect through.
async fn observe(database_url: &str) -> Result<Value, Error> {
	let database =
		DatabaseConnection::connect_postgres_with_pool_size(database_url, Some(1)).await?;
	let mut tx = begin(&database).await?;
	let runs = Query::select()
		.columns([
			Alias::new("phase"),
			Alias::new("pending"),
			Alias::new("control"),
		])
		.from(Alias::new("runs"))
		.and_where(Expr::col(Alias::new("phase")).is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
		.build(PostgresQueryBuilder);
	let mut busy = BTreeMap::new();
	let unanswered = waiting_ids(
		&mut tx,
		"human_requests",
		Expr::col(Alias::new("response")).is_null(),
	)
	.await?;
	let approvals = waiting_ids(
		&mut tx,
		"core_records",
		Expr::col(Alias::new("state"))
			.eq("pending")
			.and(Expr::col(Alias::new("expires_at")).gt(call("clock_timestamp", vec![]))),
	)
	.await?;
	let mut active_runs = 0_i64;
	for row in tx.fetch_all_generated(runs, None).await? {
		let phase = text(&row, "phase")?;
		let pending = document(&row, "pending")?;
		let control = text(&row, "control")?;
		let awaiting_person = awaited_id(&pending)
			.is_some_and(|id| unanswered.contains(id) || approvals.contains(id));
		if control != "PAUSED"
			&& !(control != "CANCELLED" && human_wait(&phase, &pending) && awaiting_person)
		{
			active_runs += 1;
		}
	}
	busy.insert("runs", active_runs);
	for (name, table, column, states) in [
		(
			"verification",
			"agent_test_sessions",
			"status",
			vec!["running"],
		),
		(
			"generation",
			"generation_requests",
			"status",
			vec!["QUEUED"],
		),
		("invocations", "invocations", "status", vec!["STARTED"]),
		(
			"participants",
			"atomic_participants",
			"phase",
			vec!["RESERVED", "PREPARED", "APPLIED"],
		),
	] {
		busy.insert(
			name,
			count(&mut tx, table, Expr::col(Alias::new(column)).is_in(states)).await?,
		);
	}
	busy.insert(
		"operations",
		count(
			&mut tx,
			"core_operations",
			Expr::col(Alias::new("state")).is_not_in([
				"completed",
				"cancelled",
				"failed",
				"uncertain",
			]),
		)
		.await?,
	);
	busy.insert(
		"transfers",
		count(
			&mut tx,
			"core_records",
			Expr::col(Alias::new("kind")).eq("transfer_out").and(
				Expr::col(Alias::new("state")).is_in(["pending", "transferring", "committing"]),
			),
		)
		.await?,
	);
	busy.insert(
		"transactions",
		count(
			&mut tx,
			"atomic_coordinators",
			Expr::col(Alias::new("complete")).eq(false),
		)
		.await?,
	);
	busy.insert(
		"leases",
		count(
			&mut tx,
			"runs",
			Expr::col(Alias::new("lease_until")).gt(call("clock_timestamp", vec![])),
		)
		.await?,
	);
	// Detect in-flight transactions after the application processes are paused,
	// including work whose durable row has not yet committed. Exclude this
	// observer connection and other databases. Unknown schema => process error.
	let condition = Condition::all()
		.add(Expr::col(Alias::new("datname")).eq(call("current_database", vec![])))
		.add(Expr::col(Alias::new("pid")).ne(call("pg_backend_pid", vec![])))
		.add(Expr::col(Alias::new("backend_type")).eq("client backend"))
		.add(Expr::col(Alias::new("state")).is_in([
			"active",
			"idle in transaction",
			"idle in transaction (aborted)",
		]));
	let query = Query::select()
		.expr_as(
			Func::count(Expr::col(ColumnRef::Asterisk).into()),
			Alias::new("total"),
		)
		.from(Alias::new("pg_stat_activity"))
		.cond_where(condition)
		.build(PostgresQueryBuilder);
	let row = tx.fetch_one_generated(query, None).await?;
	busy.insert("database_work", integer(&row, "total")?);
	// A short operation can begin and finish between timer samples. Preserve its
	// actual completion time so that it still receives a full idle interval.
	let mut last_work_completed = 0.0_f64;
	for (table, column, terminal) in [
		("runs", "phase", vec!["COMPLETED", "FAILED", "CANCELLED"]),
		(
			"core_operations",
			"state",
			vec!["completed", "failed", "cancelled", "uncertain"],
		),
		(
			"agent_test_sessions",
			"status",
			vec![
				"completed",
				"failed",
				"blocked",
				"timed_out",
				"outcome_unknown",
				"stopped",
			],
		),
	] {
		let query = Query::select()
			.expr_as(
				call(
					"date_part",
					vec![
						Expr::value("epoch").into(),
						Func::max(Expr::col(Alias::new("updated_at")).into()),
					],
				),
				Alias::new("completed"),
			)
			.from(Alias::new(table))
			.and_where(Expr::col(Alias::new(column)).is_in(terminal))
			.build(PostgresQueryBuilder);
		let row = tx.fetch_one_generated(query, None).await?;
		last_work_completed = last_work_completed.max(number(&row, "completed")?.unwrap_or(0.0));
	}
	// core_records has no completion timestamp. Summarize the durable receipts
	// so the collector can start a full idle interval when a completion is
	// first seen, even if the transfer began and finished between
	// observations. Retries and object reclamation do not turn an existing
	// receipt into new activity. Receipts have no deletion path, so only a
	// fixed-size summary leaves the database; see "Activity observation
	// contract" in enterprise/infra/gcp/helm/environment/README.md.
	let (completed_transfers_count, completed_transfers_digest) = receipt_summary(&mut tx).await?;
	tx.rollback().await?;
	Ok(json!({
		"protocol": "aidash-infra-activity/1",
		"busy": busy.values().any(|n| *n > 0),
		"counts": busy,
		"last_work_completed": last_work_completed,
		"completed_transfers_count": completed_transfers_count,
		"completed_transfers_digest": completed_transfers_digest,
	}))
}

#[tokio::main]
async fn main() {
	let observation = async { observe(&std::env::var("DATABASE_URL")?).await };
	match tokio::time::timeout(Duration::from_secs(10), observation).await {
		Ok(Ok(value)) => println!("{value}"),
		_ => {
			// Never emit credentials, SQL parameter values, or database URLs.
			eprintln!("activity unavailable; automatic stop/deployment must be deferred");
			std::process::exit(1);
		}
	}
}

#[cfg(test)]
mod tests;
