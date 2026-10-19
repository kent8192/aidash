//! Row-level policies on Tenant-owned tables (#264), as the server's login
//! sees them.
use super::native_database::{DatabaseFixture, admin_pool, database, owner_pool};
use aidash_application::tenancy::{TEST_FIXTURE, TenancyScope};
use aidash_server::apps::identity::models::TenantRetentionRule;
use aidash_server::database::scope::{SCOPE_SETTING, TENANT_SETTING};
use aidash_server::database::{native, policy, runtime};
use chrono::Utc;
use reinhardt::db::orm::Model;
use rstest::rstest;
use serde_json::Value;

/// The policy functions' message for a transaction without a usable scope.
const SCOPE_MISSING: &str = "Tenant-owned data needs a Tenant or Deployment Scope";
/// PostgreSQL's message when a written row fails `WITH CHECK`.
const CHECK_FAILED: &str = "row-level security";

async fn server_pool(f: &DatabaseFixture) -> native::Pool {
	native::Pool::from(
		runtime::connect(&f.url, 2)
			.await
			.unwrap()
			.into_postgres()
			.unwrap(),
	)
}

/// A disposable owner-created table carrying the real template, in its own
/// schema so that the classification test never sees it.
struct Probe {
	owner: sqlx::PgPool,
	schema: String,
}

impl Probe {
	async fn new(pool: &native::Pool) -> Self {
		let owner = owner_pool(pool.driver()).await;
		let schema = format!("rls_probe_{}", uuid::Uuid::new_v4().simple());
		let table = format!("{schema}.probe");
		sqlx::raw_sql(&format!(
			"CREATE SCHEMA {schema}; GRANT USAGE ON SCHEMA {schema} TO aidash_server; \
			 CREATE TABLE {table} (id integer PRIMARY KEY, tenant text); \
			 GRANT SELECT, INSERT, UPDATE, DELETE ON {table} TO aidash_server; {}",
			policy::enable_table(&table)
		))
		.execute(&owner)
		.await
		.unwrap();
		Self { owner, schema }
	}

	fn table(&self) -> String {
		format!("{}.probe", self.schema)
	}

	async fn drop(self) {
		sqlx::raw_sql(&format!("DROP SCHEMA {} CASCADE", self.schema))
			.execute(&self.owner)
			.await
			.unwrap();
		self.owner.close().await;
	}
}

async fn tenants(pool: &native::Pool, scope: &TenancyScope, table: &str) -> Vec<Option<String>> {
	let sql = format!("SELECT tenant FROM {table} ORDER BY id");
	pool.scoped(scope, async |tx| {
		native::query_scalar::<Option<String>>(&sql)
			.scalar_all(&mut **tx)
			.await
	})
	.await
	.unwrap()
}

async fn statement(
	pool: &native::Pool,
	scope: &TenancyScope,
	sql: &str,
) -> aidash_server::Result<u64> {
	pool.scoped(scope, async |tx| {
		Ok(native::query(sql).execute(&mut **tx).await?.rows_affected())
	})
	.await
}

#[rstest]
#[tokio::test]
async fn a_tenant_scope_reaches_only_its_own_rows(#[future] database: DatabaseFixture) {
	// Arrange: rows of two Tenants and two rows without a Tenant.
	let f = database.await;
	let pool = server_pool(&f).await;
	let probe = Probe::new(&pool).await;
	let table = probe.table();
	let acme = TenancyScope::tenant("acme").unwrap();
	statement(
		&pool,
		&TEST_FIXTURE,
		&format!("INSERT INTO {table} VALUES (1, 'acme'), (2, 'beta'), (3, NULL), (4, '')"),
	)
	.await
	.unwrap();
	// Act
	let seen = tenants(&pool, &acme, &table).await;
	let deployment = tenants(&pool, &TEST_FIXTURE, &table).await;
	let foreign_insert = statement(
		&pool,
		&acme,
		&format!("INSERT INTO {table} VALUES (5, 'beta')"),
	)
	.await;
	let moved = statement(
		&pool,
		&acme,
		&format!("UPDATE {table} SET tenant = 'beta' WHERE id = 1"),
	)
	.await;
	let foreign_update = statement(
		&pool,
		&acme,
		&format!("UPDATE {table} SET id = id WHERE tenant = 'beta'"),
	)
	.await
	.unwrap();
	let foreign_delete = statement(&pool, &acme, &format!("DELETE FROM {table} WHERE id <> 1"))
		.await
		.unwrap();
	let own_insert = statement(
		&pool,
		&acme,
		&format!("INSERT INTO {table} VALUES (6, 'acme')"),
	)
	.await
	.unwrap();
	// Assert: a Tenant scope sees its rows only, and rows without a Tenant
	// only in a Deployment Scope.
	assert_eq!(seen, [Some("acme".to_owned())]);
	assert_eq!(
		deployment,
		[
			Some("acme".to_owned()),
			Some("beta".to_owned()),
			None,
			Some(String::new())
		]
	);
	// Writing another Tenant's value fails; targeting its rows reaches none.
	let error = foreign_insert.unwrap_err().to_string();
	assert!(error.contains(CHECK_FAILED), "{error}");
	let error = moved.unwrap_err().to_string();
	assert!(error.contains(CHECK_FAILED), "{error}");
	assert_eq!((foreign_update, foreign_delete, own_insert), (0, 0, 1));
	assert_eq!(tenants(&pool, &TEST_FIXTURE, &table).await.len(), 5);
	probe.drop().await;
}

#[rstest]
#[tokio::test]
async fn work_without_a_usable_scope_cannot_reach_tenant_owned_rows(
	#[future] database: DatabaseFixture,
) {
	// Arrange
	let f = database.await;
	let pool = server_pool(&f).await;
	let probe = Probe::new(&pool).await;
	let table = probe.table();
	let select = format!("SELECT count(*) FROM {table}");
	statement(
		&pool,
		&TEST_FIXTURE,
		&format!("INSERT INTO {table} VALUES (1, 'acme')"),
	)
	.await
	.unwrap();
	// Act: the no-Tenant scope, a statement outside any transaction, the
	// owner's own login, and a hand-forged Tenant scope without a name, which
	// `scope.rs` cannot produce.
	let no_tenant = statement(&pool, &TenancyScope::NoTenant, &select).await;
	let unscoped = sqlx::query(&select).fetch_one(pool.driver()).await;
	let owner = sqlx::query(&select).fetch_one(&probe.owner).await;
	let mut forged = pool.driver().begin().await.unwrap();
	sqlx::query("SELECT set_config($1, 'tenant', true), set_config($2, '', true)")
		.bind(SCOPE_SETTING)
		.bind(TENANT_SETTING)
		.execute(&mut *forged)
		.await
		.unwrap();
	let nameless = sqlx::query(&select).fetch_one(&mut *forged).await;
	forged.rollback().await.unwrap();
	// Assert
	let error = no_tenant.unwrap_err().to_string();
	assert!(error.contains(SCOPE_MISSING), "{error}");
	for (case, result) in [
		("unscoped", unscoped),
		("owner", owner),
		("nameless", nameless),
	] {
		let error = result.expect_err(case);
		let database = error.as_database_error().expect(case);
		assert_eq!(
			database.code().as_deref(),
			Some(policy::SCOPE_MISSING),
			"{case}"
		);
		assert!(!database.message().contains("acme"), "{case}");
	}
	probe.drop().await;
}

fn rule(tenant: &str) -> TenantRetentionRule {
	TenantRetentionRule {
		tenant: tenant.to_owned(),
		snapshot_after_run_seconds: 60,
		snapshot_max_age_seconds: 120,
		updated_at: Utc::now(),
		updated_by: "deployment_administrator".to_owned(),
	}
}

async fn rules(pool: &native::Pool, scope: &TenancyScope) -> aidash_server::Result<Vec<String>> {
	pool.atomic(scope, async |tx| {
		Ok(TenantRetentionRule::objects()
			.all()
			.all_with_executor(tx)
			.await
			.map_err(reinhardt::core::exception::Error::from)?
			.into_iter()
			.map(|rule| rule.tenant)
			.collect())
	})
	.await
}

#[rstest]
#[tokio::test]
async fn orm_work_on_a_tenant_owned_table_stays_in_its_tenant(#[future] database: DatabaseFixture) {
	// Arrange
	let f = database.await;
	let pool = server_pool(&f).await;
	let acme = TenancyScope::tenant("acme").unwrap();
	for tenant in ["acme", "beta", "gamma"] {
		f.tenant(tenant).await;
	}
	for tenant in ["acme", "beta"] {
		pool.atomic(&TEST_FIXTURE, async |tx| {
			TenantRetentionRule::objects()
				.create_with_conn(tx, &rule(tenant))
				.await?;
			Ok(())
		})
		.await
		.unwrap();
	}
	// Act
	let seen = rules(&pool, &acme).await.unwrap();
	let foreign = pool
		.atomic(&acme, async |tx| {
			TenantRetentionRule::objects()
				.create_with_conn(tx, &rule("gamma"))
				.await?;
			Ok(())
		})
		.await;
	let no_tenant = rules(&pool, &TenancyScope::NoTenant).await;
	// Assert
	assert_eq!(seen, ["acme"]);
	let error = foreign.unwrap_err().to_string();
	assert!(error.contains(CHECK_FAILED), "{error}");
	let error = no_tenant.unwrap_err().to_string();
	assert!(error.contains(SCOPE_MISSING), "{error}");
	let mut all = rules(&pool, &TEST_FIXTURE).await.unwrap();
	all.sort();
	assert_eq!(all, ["acme", "beta"]);
}

/// A row of each Tenant in a table that gained its `tenant` column in #265. The
/// superuser writes them with foreign keys off, so a row needs none of its
/// parents; what the test asks is how the policy treats the row.
#[rstest]
#[case::runs(
	"runs",
	"id, tenant, task_id, workspace_id, home_node, agent_id, agent_version",
	"gen_random_uuid(), $1, gen_random_uuid(), gen_random_uuid(), 'n', 'a', '1'"
)]
#[case::messages(
	"messages",
	"id, tenant, workspace_id, sender, content",
	"gen_random_uuid(), $1, gen_random_uuid(), 'alice', 'text'"
)]
#[case::events(
	"events",
	"id, tenant, node_id, kind, data",
	"gen_random_uuid(), $1, 'n', 'k', '{}'"
)]
#[case::memory_receipts(
	"memory_receipts",
	"operation_id, tenant, bank_id, digest, outcome, created_at",
	"gen_random_uuid(), $1, gen_random_uuid(), 'd', '{}', now()"
)]
#[tokio::test]
async fn tables_that_gained_a_tenant_hide_other_tenants_and_refuse_their_values(
	#[future] database: DatabaseFixture,
	#[case] table: &str,
	#[case] columns: &str,
	#[case] values: &str,
) {
	// Arrange
	let f = database.await;
	let pool = server_pool(&f).await;
	let admin = admin_pool(pool.driver()).await;
	let insert = format!("INSERT INTO {table} ({columns}) VALUES ({values})");
	let mut seed = admin.begin().await.unwrap();
	sqlx::query("SET LOCAL session_replication_role = replica")
		.execute(&mut *seed)
		.await
		.unwrap();
	for tenant in ["acme", "beta"] {
		sqlx::query(&insert)
			.bind(tenant)
			.execute(&mut *seed)
			.await
			.unwrap();
	}
	seed.commit().await.unwrap();
	let acme = TenancyScope::tenant("acme").unwrap();
	let count = format!("SELECT count(*) FROM {table}");
	let own = format!("SELECT count(*) FROM {table} WHERE tenant = 'acme'");
	let foreign = format!("SELECT count(*) FROM {table} WHERE tenant = 'beta'");
	// Act
	let seen = pool
		.scoped(&acme, async |tx| {
			native::query_scalar::<i64>(&count)
				.scalar_one(&mut **tx)
				.await
		})
		.await
		.unwrap();
	let own_rows = pool
		.scoped(&acme, async |tx| {
			native::query_scalar::<i64>(&own)
				.scalar_one(&mut **tx)
				.await
		})
		.await
		.unwrap();
	let foreign_rows = pool
		.scoped(&acme, async |tx| {
			native::query_scalar::<i64>(&foreign)
				.scalar_one(&mut **tx)
				.await
		})
		.await
		.unwrap();
	let everything = pool
		.scoped(&TEST_FIXTURE, async |tx| {
			native::query_scalar::<i64>(&count)
				.scalar_one(&mut **tx)
				.await
		})
		.await
		.unwrap();
	let written = pool
		.scoped(&acme, async |tx| {
			native::query(&insert.replace("$1", "'beta'"))
				.execute(&mut **tx)
				.await?;
			Ok(())
		})
		.await;
	// Assert: a read in another Tenant's scope returns nothing, and a write of
	// another Tenant's value is refused.
	assert_eq!((seen, own_rows, foreign_rows, everything), (1, 1, 0, 2));
	let error = written.unwrap_err().to_string();
	assert!(error.contains(CHECK_FAILED), "{table}: {error}");
	admin.close().await;
}

/// Every plan node of a JSON `EXPLAIN`, depth first.
fn nodes(plan: &Value, found: &mut Vec<Value>) {
	found.push(plan.clone());
	for child in plan["Plans"].as_array().into_iter().flatten() {
		nodes(child, found);
	}
}

#[rstest]
#[case::run_load_grant(
	"SELECT EXISTS (SELECT 1 FROM authorization_execution \
	 WHERE run_id = '00000000-0000-0000-0000-000000000001' \
	 AND workspace_id = '00000000-0000-0000-0000-000000000002')",
	"authorization_execution_pkey"
)]
#[case::policy_revision(
	"SELECT * FROM authorization_revisions WHERE tenant = 'acme' AND revision = 1",
	"authorization_revisions_pkey"
)]
#[case::approval(
	"SELECT * FROM core_records WHERE id = '00000000-0000-0000-0000-000000000001'",
	"core_records_pkey"
)]
#[case::generation_request(
	"SELECT * FROM generation_requests WHERE id = '00000000-0000-0000-0000-000000000001'",
	"generation_requests_pkey"
)]
#[case::event_replay(
	"SELECT * FROM events WHERE workspace_id = '00000000-0000-0000-0000-000000000001' \
	 AND sequence > 0 ORDER BY sequence LIMIT 100",
	"events_workspace"
)]
#[case::server_sent_event_read(
	"SELECT * FROM events WHERE sequence > 0 ORDER BY sequence LIMIT 100",
	"events_pkey"
)]
#[case::activation_claim(
	"SELECT generation FROM run_activations WHERE state <> 'settled' \
	 AND due_at <= CURRENT_TIMESTAMP ORDER BY due_at, generation LIMIT 64 \
	 FOR UPDATE SKIP LOCKED",
	"activation_due"
)]
#[tokio::test]
async fn hot_reads_use_their_index_and_evaluate_the_policy_once(
	#[future] database: DatabaseFixture,
	#[case] query: &str,
	#[case] index: &str,
) {
	// Arrange: the planner may prefer a sequential scan on an empty table, so
	// disable it to ask whether the policy leaves the index usable.
	let f = database.await;
	let pool = server_pool(&f).await;
	let explain = format!("EXPLAIN (VERBOSE, FORMAT JSON) {query}");
	// Act
	let plan = pool
		.scoped(&TenancyScope::tenant("acme").unwrap(), async |tx| {
			native::query("SET LOCAL enable_seqscan = off")
				.execute(&mut **tx)
				.await?;
			native::query_scalar::<Value>(&explain)
				.scalar_one(&mut **tx)
				.await
		})
		.await
		.unwrap();
	// Assert
	let mut all = Vec::new();
	nodes(&plan[0]["Plan"], &mut all);
	assert!(
		all.iter().any(|node| node["Index Name"] == index),
		"{index} unused: {plan:#}"
	);
	let (init, rest): (Vec<_>, Vec<_>) = all
		.iter()
		.partition(|node| node["Parent Relationship"] == "InitPlan");
	let calls = |node: &Value, function: &str| node.to_string().contains(function);
	for function in ["aidash_deployment_scope()", "aidash_scope_tenant()"] {
		assert!(
			init.iter().any(|node| calls(node, function)),
			"{function} is not an InitPlan: {plan:#}"
		);
		assert!(
			rest.iter().all(|node| {
				let mut own = (*node).clone();
				own.as_object_mut().unwrap().remove("Plans");
				!calls(&own, function)
			}),
			"{function} runs per row: {plan:#}"
		);
	}
}
