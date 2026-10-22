use super::*;
use reinhardt_query::{ColumnDef, Value as QueryParameter};

#[test]
fn only_explicit_human_waits_can_idle() {
	let human = json!({"data":{"reason":"human","request_id":"id","wake_at":"expiry"}});
	let approval =
		json!({"data":{"reason":"approval","approval":{"source":"capability","approval_id":"id"}}});
	let home = json!({"data":{"reason":"approval","approval":{"source":"home","request_id":"id"}}});
	assert!(human_wait("WAITING", &human));
	assert!(human_wait("WAITING", &approval));
	assert_eq!(awaited_id(&approval), Some("id"));
	assert!(!human_wait("WAITING", &home));
	assert!(!human_wait("WAITING", &json!({"data":{"wake_at":"later"}})));
	assert!(!human_wait("THINKING", &human));
	assert!(!human_wait(
		"WAITING",
		&json!({"data":{"reason":"human","request_id":null}})
	));
	assert!(!human_wait("FUTURE_PHASE", &json!({})));
}

/// The pinned Reinhardt revision of a manifest's `rev = "…"` entries.
fn revisions(manifest: &str) -> HashSet<&str> {
	manifest
		.lines()
		.filter(|line| line.contains("kent8192/reinhardt-web"))
		.filter_map(|line| line.split("rev = \"").nth(1)?.split('"').next())
		.collect()
}

/// The observer speaks the server's tenancy contract by hand, so it must be
/// built against the Reinhardt revision the server uses.
#[test]
fn reinhardt_is_pinned_to_the_server_revision() {
	let server = revisions(include_str!("../../../../Cargo.toml"));
	let observer = revisions(include_str!("../Cargo.toml"));
	assert_eq!(server.len(), 1, "{server:?}");
	assert_eq!(observer, server);
}

async fn run(
	database: &DatabaseConnection,
	statement: &impl QueryStatementBuilder,
) -> Result<(), Error> {
	database
		.execute_generated(statement.build(PostgresQueryBuilder), None)
		.await?;
	Ok(())
}

/// The server's policy migrations, for the observed tables that carry a
/// Tenant. Each table's block starts at its `ENABLE` statement.
fn policies(tables: &[&str]) -> String {
	let assets = [
		include_str!(
			"../../../../server/migrations/execution/sql/forward/0036_row_level_security.sql"
		),
		include_str!(
			"../../../../server/migrations/registry/sql/forward/0033_row_level_security.sql"
		),
	];
	let mut selected = String::new();
	for asset in assets {
		for block in asset.split("ALTER TABLE public.").skip(1) {
			let block = format!("ALTER TABLE public.{block}");
			let table = block["ALTER TABLE public.".len()..]
				.split_whitespace()
				.next()
				.unwrap();
			if tables.contains(&table) {
				selected.push_str(&block);
			}
		}
	}
	selected
}

#[tokio::test]
#[ignore = "requires a fresh disposable PostgreSQL aidash_observer_test database"]
#[allow(clippy::disallowed_methods)] // The fixture seeds as the database superuser.
async fn database_snapshot_covers_all_rows_and_fails_closed() -> Result<(), Error> {
	let url = std::env::var("AIDASH_OBSERVER_TEST_DATABASE_URL")?;
	assert!(
		url.ends_with("/aidash_observer_test"),
		"use only the disposable test database"
	);
	// The deployed observer's own role; its grants come from
	// infra/gcp/control/kube.py observer_sql, run here as on the cluster.
	let role_url = std::env::var("AIDASH_OBSERVER_TEST_ROLE_URL")?;
	assert!(
		role_url.ends_with("/aidash_observer_test"),
		"use only the disposable test database"
	);
	let database = DatabaseConnection::connect_postgres(&url).await?;
	let admin = database.into_postgres().ok_or("not PostgreSQL")?;
	// The tables that carry a Tenant get the server's real policy functions and
	// forced policies, so the role reads them only in its Deployment Scope.
	let tenant_owned = [
		"core_records",
		"core_operations",
		"agent_test_sessions",
		"generation_requests",
	];
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
				("tenant", "text"),
			],
		),
		(
			"core_operations",
			vec![
				("state", "text"),
				("updated_at", "time"),
				("tenant", "text"),
			],
		),
		(
			"agent_test_sessions",
			vec![
				("status", "text"),
				("updated_at", "time"),
				("tenant", "text"),
			],
		),
		(
			"generation_requests",
			vec![("status", "text"), ("tenant", "text")],
		),
		("invocations", vec![("status", "text")]),
		("atomic_participants", vec![("phase", "text")]),
		("atomic_coordinators", vec![("complete", "bool")]),
	] {
		let mut query = Query::create_table();
		query.table(Alias::new(table));
		for (name, kind) in columns {
			let column = ColumnDef::new(Alias::new(name));
			query.col(match kind {
				"uuid" => column.uuid(),
				"json" => column.jsonb(),
				"time" => column.timestamp_with_time_zone(),
				"bool" => column.boolean(),
				_ => column.text(),
			});
		}
		run(&database, &query).await?;
	}
	sqlx::raw_sql(include_str!(
		"../../../../server/migrations/identity/sql/forward/0028_tenancy_policy_functions.sql"
	))
	.execute(&admin)
	.await?;
	let policies = policies(&tenant_owned);
	assert_eq!(
		policies.matches("CREATE POLICY").count(),
		tenant_owned.len()
	);
	sqlx::raw_sql(&policies).execute(&admin).await?;
	sqlx::raw_sql(&std::env::var("AIDASH_OBSERVER_TEST_ROLE_SQL")?)
		.execute(&admin)
		.await?;
	let mut insert = Query::insert();
	insert
		.into_table(Alias::new("runs"))
		.columns(["phase", "control", "pending"].map(Alias::new));
	for _ in 0..600 {
		insert.values(vec![
			"THINKING".into(),
			"RUNNING".into(),
			QueryParameter::from(json!({})),
		])?;
	}
	run(&database, &insert).await?;
	let snapshot = observe(&url).await?;
	assert_eq!(snapshot["counts"]["runs"], 600);
	assert_eq!(snapshot["busy"], true);
	let mut update = Query::update();
	update
		.table(Alias::new("runs"))
		.value(Alias::new("phase"), "COMPLETED")
		.value_expr(Alias::new("updated_at"), Expr::current_timestamp());
	run(&database, &update).await?;
	assert_eq!(observe(&url).await?["busy"], false);
	assert!(
		observe(&url).await?["last_work_completed"]
			.as_f64()
			.unwrap() > 1_000_000_000.0
	);
	let previous = observe(&url).await?["last_work_completed"]
		.as_f64()
		.unwrap();
	// Each terminal outcome can begin and finish between observation samples.
	for (index, status) in [
		"completed",
		"failed",
		"blocked",
		"timed_out",
		"outcome_unknown",
		"stopped",
	]
	.into_iter()
	.enumerate()
	{
		let completed = previous + 10.0 + index as f64;
		let mut insert = Query::insert();
		insert
			.into_table(Alias::new("agent_test_sessions"))
			.columns(["status", "updated_at", "tenant"].map(Alias::new))
			.values_expr(vec![
				Expr::value(status).into(),
				call("to_timestamp", vec![Expr::value(completed).into()]),
				Expr::value("acme").into(),
			])?;
		run(&database, &insert).await?;
		let snapshot = observe(&url).await?;
		assert_eq!(snapshot["busy"], false, "terminal test status {status}");
		assert_eq!(
			snapshot["last_work_completed"].as_f64().unwrap(),
			completed,
			"terminal test status {status}"
		);
	}
	let mut insert = Query::insert();
	insert
		.into_table(Alias::new("agent_test_sessions"))
		.columns(["status", "updated_at", "tenant"].map(Alias::new))
		.values_expr(vec![
			Expr::value("running").into(),
			call("to_timestamp", vec![Expr::value(previous + 100.0).into()]),
			Expr::value("beta").into(),
		])?;
	run(&database, &insert).await?;
	let snapshot = observe(&url).await?;
	assert_eq!(snapshot["counts"]["verification"], 1);
	assert_eq!(
		snapshot["last_work_completed"].as_f64().unwrap(),
		previous + 15.0
	);
	let mut delete = Query::delete();
	delete.from_table(Alias::new("agent_test_sessions"));
	run(&database, &delete).await?;
	for (id, kind, state) in [
		(
			"00000000-0000-0000-0000-000000000001",
			"transfer_out",
			"delivered",
		),
		(
			"00000000-0000-0000-0000-000000000002",
			"transfer_in",
			"committed",
		),
		(
			"00000000-0000-0000-0000-000000000003",
			"reference",
			"committed",
		),
		(
			"00000000-0000-0000-0000-000000000004",
			"transfer_out",
			"blocked",
		),
	] {
		let mut insert = Query::insert();
		insert
			.into_table(Alias::new("core_records"))
			.columns(["id", "kind", "state", "tenant"].map(Alias::new))
			.values_expr(vec![
				Expr::value(id).cast_as("uuid"),
				Expr::value(kind).into(),
				Expr::value(state).into(),
				Expr::value("acme").into(),
			])?;
		run(&database, &insert).await?;
	}
	let snapshot = observe(&url).await?;
	assert_eq!(snapshot["busy"], false);
	assert_eq!(snapshot["completed_transfers_count"], 2);
	let small = snapshot["completed_transfers_digest"]
		.as_str()
		.unwrap()
		.to_owned();
	assert!(small.len() == 64 && small.bytes().all(|b| b.is_ascii_hexdigit()));
	assert!(snapshot.get("completed_transfers").is_none());
	// Receipts only accumulate: a very large set must still produce a
	// fixed-size observation that a new receipt visibly changes.
	sqlx::raw_sql(
		"INSERT INTO core_records (id, kind, state) \
		 SELECT gen_random_uuid(), 'transfer_out', 'delivered' FROM generate_series(1, 50000)",
	)
	.execute(&admin)
	.await?;
	let large = observe(&url).await?;
	assert_eq!(large["completed_transfers_count"], 50_002);
	assert_ne!(large["completed_transfers_digest"], small);
	let bytes = serde_json::to_string(&large)?.len();
	assert!(
		bytes < 1024,
		"observation grew with the receipt set: {bytes} bytes"
	);
	assert_eq!(
		observe(&url).await?["completed_transfers_digest"],
		large["completed_transfers_digest"],
		"an unchanged set must keep its digest"
	);
	let mut insert = Query::insert();
	insert
		.into_table(Alias::new("core_records"))
		.columns(["id", "kind", "state"].map(Alias::new))
		.values_expr(vec![
			Expr::value("00000000-0000-0000-0000-000000000005").cast_as("uuid"),
			Expr::value("transfer_in").into(),
			Expr::value("committed").into(),
		])?;
	run(&database, &insert).await?;
	let renewed = observe(&url).await?;
	assert_eq!(renewed["completed_transfers_count"], 50_003);
	assert_ne!(
		renewed["completed_transfers_digest"], large["completed_transfers_digest"],
		"a new receipt must change the digest so idle is renewed"
	);
	// The role sees the same evidence, including other sessions' open
	// transactions, which pg_stat_activity hides without pg_read_all_stats,
	// and rows of every Tenant and of none, through its Deployment Scope.
	let role = observe(&role_url).await?;
	assert_eq!(role["busy"], snapshot["busy"]);
	assert_eq!(role["counts"], snapshot["counts"]);
	assert_eq!(role["completed_transfers_count"], 50_003);
	let mut open = admin.begin().await?;
	sqlx::query("SELECT 1").execute(&mut *open).await?;
	let role = observe(&role_url).await?;
	assert_eq!(role["counts"]["database_work"], 1);
	assert_eq!(role["busy"], true);
	open.rollback().await?;
	assert_eq!(observe(&role_url).await?["busy"], false);
	// It can neither change retained data nor read any other table, and
	// without its scope the forced policy refuses even its own reads.
	let observer = sqlx::PgPool::connect(&role_url).await?;
	assert!(
		sqlx::raw_sql("DELETE FROM core_records")
			.execute(&observer)
			.await
			.is_err()
	);
	let unscoped = sqlx::raw_sql("SELECT count(*) FROM core_records")
		.execute(&observer)
		.await
		.expect_err("a read without a scope must fail");
	assert_eq!(
		unscoped
			.as_database_error()
			.and_then(|error| error.code())
			.as_deref(),
		Some("TS001")
	);
	sqlx::raw_sql("CREATE TABLE unrelated_fixture (secret text)")
		.execute(&admin)
		.await?;
	assert!(
		sqlx::raw_sql("SELECT secret FROM unrelated_fixture")
			.execute(&observer)
			.await
			.is_err()
	);
	observer.close().await;
	let mut insert = Query::insert();
	insert
		.into_table(Alias::new("core_operations"))
		.columns([Alias::new("state")])
		.values(vec!["future_active_state".into()])?;
	run(&database, &insert).await?;
	assert_eq!(observe(&url).await?["busy"], true);
	let mut drop = Query::drop_table();
	drop.table(Alias::new("agent_test_sessions"));
	run(&database, &drop).await?;
	assert!(
		observe(&url).await.is_err(),
		"missing verification state must never mean idle"
	);
	Ok(())
}
