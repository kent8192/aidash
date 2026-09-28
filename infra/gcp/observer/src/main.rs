//! Infrastructure-only, read-only activity snapshot. No application writes or migrations.
use sea_orm::{
	AccessMode, ConnectOptions, ConnectionTrait, Database, DatabaseTransaction, DbBackend,
	IsolationLevel, TransactionTrait,
	sea_query::{Alias, Asterisk, Condition, Expr, Func, Query, SimpleExpr},
};
use serde_json::{Value, json};
use std::{
	collections::{BTreeMap, HashSet},
	time::Duration,
};

type Error = Box<dyn std::error::Error + Send + Sync>;

fn human_wait(phase: &str, pending: &Value) -> bool {
	// A timed WAITING phase is work. A human/approval wait can also carry an
	// expiry wake_at; that does not extend the application's approval deadline.
	phase == "WAITING"
		&& ["human_request_id", "core_approval_id"]
			.iter()
			.any(|key| pending.get(key).is_some_and(|v| v.is_string()))
}

async fn count(tx: &DatabaseTransaction, table: &str, condition: SimpleExpr) -> Result<i64, Error> {
	let query = Query::select()
		.expr_as(Func::count(Expr::col(Asterisk)), Alias::new("total"))
		.from(Alias::new(table))
		.and_where(condition)
		.to_owned();
	let row = tx
		.query_one(DbBackend::Postgres.build(&query))
		.await?
		.ok_or("missing count")?;
	Ok(row.try_get("", "total")?)
}

async fn waiting_ids(
	tx: &DatabaseTransaction,
	table: &str,
	condition: SimpleExpr,
) -> Result<HashSet<String>, Error> {
	let query = Query::select()
		.expr_as(
			Expr::col(Alias::new("id")).cast_as(Alias::new("text")),
			Alias::new("id"),
		)
		.from(Alias::new(table))
		.and_where(condition)
		.to_owned();
	let mut ids = HashSet::new();
	for row in tx.query_all(DbBackend::Postgres.build(&query)).await? {
		ids.insert(row.try_get("", "id")?);
	}
	Ok(ids)
}

async fn observe(database_url: &str) -> Result<Value, Error> {
	let mut options = ConnectOptions::new(database_url);
	options
		.max_connections(1)
		.sqlx_logging(false)
		.connect_timeout(Duration::from_secs(5))
		.acquire_timeout(Duration::from_secs(5));
	let database = Database::connect(options).await?;
	let tx = database
		.begin_with_config(
			Some(IsolationLevel::RepeatableRead),
			Some(AccessMode::ReadOnly),
		)
		.await?;
	let query = Query::select()
		.columns([
			Alias::new("phase"),
			Alias::new("pending"),
			Alias::new("control"),
		])
		.from(Alias::new("runs"))
		.and_where(Expr::col(Alias::new("phase")).is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
		.to_owned();
	let mut busy = BTreeMap::new();
	let unanswered = waiting_ids(
		&tx,
		"human_requests",
		Expr::col(Alias::new("response")).is_null(),
	)
	.await?;
	let approvals = waiting_ids(
		&tx,
		"core_records",
		Expr::col(Alias::new("state"))
			.eq("pending")
			.and(Expr::col(Alias::new("expires_at")).gt(Func::cust(Alias::new("clock_timestamp")))),
	)
	.await?;
	let mut active_runs = 0_i64;
	for row in tx.query_all(DbBackend::Postgres.build(&query)).await? {
		let phase: String = row.try_get("", "phase")?;
		let pending: Value = row.try_get("", "pending")?;
		let control: String = row.try_get("", "control")?;
		let awaiting_person = pending["human_request_id"]
			.as_str()
			.is_some_and(|id| unanswered.contains(id))
			|| pending["core_approval_id"]
				.as_str()
				.is_some_and(|id| approvals.contains(id));
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
			count(&tx, table, Expr::col(Alias::new(column)).is_in(states)).await?,
		);
	}
	busy.insert(
		"operations",
		count(
			&tx,
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
			&tx,
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
			&tx,
			"atomic_coordinators",
			Expr::col(Alias::new("complete")).eq(false),
		)
		.await?,
	);
	busy.insert(
		"leases",
		count(
			&tx,
			"runs",
			Expr::col(Alias::new("lease_until")).gt(Func::cust(Alias::new("clock_timestamp"))),
		)
		.await?,
	);
	// Detect in-flight transactions after the application processes are paused,
	// including work whose durable row has not yet committed. Exclude this
	// observer connection and other databases. Unknown schema => process error.
	let condition = Condition::all()
		.add(Expr::col(Alias::new("datname")).eq(Func::cust(Alias::new("current_database"))))
		.add(Expr::col(Alias::new("pid")).ne(Func::cust(Alias::new("pg_backend_pid"))))
		.add(Expr::col(Alias::new("backend_type")).eq("client backend"))
		.add(Expr::col(Alias::new("state")).is_in([
			"active",
			"idle in transaction",
			"idle in transaction (aborted)",
		]));
	let query = Query::select()
		.expr_as(Func::count(Expr::col(Asterisk)), Alias::new("total"))
		.from(Alias::new("pg_stat_activity"))
		.cond_where(condition)
		.to_owned();
	let row = tx
		.query_one(DbBackend::Postgres.build(&query))
		.await?
		.ok_or("missing database activity")?;
	busy.insert("database_work", row.try_get::<i64>("", "total")?);
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
			vec!["completed", "failed", "cancelled"],
		),
	] {
		let query = Query::select()
			.expr_as(
				Func::cust(Alias::new("date_part")).args([
					Expr::val("epoch").into(),
					Func::max(Expr::col(Alias::new("updated_at"))).into(),
				]),
				Alias::new("completed"),
			)
			.from(Alias::new(table))
			.and_where(Expr::col(Alias::new(column)).is_in(terminal))
			.to_owned();
		let row = tx
			.query_one(DbBackend::Postgres.build(&query))
			.await?
			.ok_or("missing completion time")?;
		last_work_completed =
			last_work_completed.max(row.try_get::<Option<f64>>("", "completed")?.unwrap_or(0.0));
	}
	tx.rollback().await?;
	Ok(
		json!({"protocol":"aidash-infra-activity/1", "busy":busy.values().any(|n| *n > 0), "counts":busy, "last_work_completed":last_work_completed}),
	)
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
mod tests {
	use super::*;
	#[test]
	fn only_explicit_human_waits_can_idle() {
		assert!(human_wait(
			"WAITING",
			&json!({"human_request_id":"id", "wake_at":"expiry"})
		));
		assert!(human_wait("WAITING", &json!({"core_approval_id":"id"})));
		assert!(!human_wait("WAITING", &json!({"wake_at":"later"})));
		assert!(!human_wait(
			"THINKING",
			&json!({"human_request_id":"old-id"})
		));
		assert!(!human_wait("WAITING", &json!({"human_request_id":null})));
		assert!(!human_wait("FUTURE_PHASE", &json!({})));
	}

	#[tokio::test]
	#[ignore = "requires a fresh disposable PostgreSQL aidash_observer_test database"]
	async fn database_snapshot_covers_all_rows_and_fails_closed() -> Result<(), Error> {
		use sea_orm::sea_query::{ColumnDef, Table};
		let url = std::env::var("AIDASH_OBSERVER_TEST_DATABASE_URL")?;
		assert!(
			url.ends_with("/aidash_observer_test"),
			"use only the disposable test database"
		);
		let database = Database::connect(&url).await?;
		for (table, columns) in [
			(
				"runs",
				vec![
					("phase", "text"),
					("control", "text"),
					("pending", "json"),
					("lease_until", "time"),
					("updated_at", "time"),
				],
			),
			("human_requests", vec![("id", "uuid"), ("response", "json")]),
			(
				"core_records",
				vec![
					("id", "uuid"),
					("state", "text"),
					("kind", "text"),
					("expires_at", "time"),
				],
			),
			(
				"core_operations",
				vec![("state", "text"), ("updated_at", "time")],
			),
			(
				"agent_test_sessions",
				vec![("status", "text"), ("updated_at", "time")],
			),
			("generation_requests", vec![("status", "text")]),
			("invocations", vec![("status", "text")]),
			("atomic_participants", vec![("phase", "text")]),
			("atomic_coordinators", vec![("complete", "bool")]),
		] {
			let mut query = Table::create();
			query.table(Alias::new(table));
			for (name, kind) in columns {
				let mut column = ColumnDef::new(Alias::new(name));
				match kind {
					"uuid" => column.uuid(),
					"json" => column.json_binary(),
					"time" => column.timestamp_with_time_zone(),
					"bool" => column.boolean(),
					_ => column.text(),
				};
				query.col(&mut column);
			}
			database.execute(DbBackend::Postgres.build(&query)).await?;
		}
		let mut insert = Query::insert();
		insert
			.into_table(Alias::new("runs"))
			.columns(["phase", "control", "pending"].map(Alias::new));
		for _ in 0..600 {
			insert.values(["THINKING".into(), "RUNNING".into(), json!({}).into()])?;
		}
		database.execute(DbBackend::Postgres.build(&insert)).await?;
		let snapshot = observe(&url).await?;
		assert_eq!(snapshot["counts"]["runs"], 600);
		assert_eq!(snapshot["busy"], true);
		let update = Query::update()
			.table(Alias::new("runs"))
			.value(Alias::new("phase"), "COMPLETED")
			.value(Alias::new("updated_at"), Expr::current_timestamp())
			.to_owned();
		database.execute(DbBackend::Postgres.build(&update)).await?;
		assert_eq!(observe(&url).await?["busy"], false);
		assert!(
			observe(&url).await?["last_work_completed"]
				.as_f64()
				.unwrap() > 1_000_000_000.0
		);
		let insert = Query::insert()
			.into_table(Alias::new("core_operations"))
			.columns([Alias::new("state")])
			.values(["future_active_state".into()])?
			.to_owned();
		database.execute(DbBackend::Postgres.build(&insert)).await?;
		assert_eq!(observe(&url).await?["busy"], true);
		let drop = Table::drop()
			.table(Alias::new("agent_test_sessions"))
			.to_owned();
		database.execute(DbBackend::Postgres.build(&drop)).await?;
		assert!(
			observe(&url).await.is_err(),
			"missing verification state must never mean idle"
		);
		Ok(())
	}
}
