use super::*;
use crate::apps::execution::test_database::{DatabaseFixture, database};
use crate::egress_tests::{settings, tenant_roots};
use aidash_application::egress::Admission;
use aidash_domain::egress::{AllowedHosts, AnomalyRule, DailyBudget};

async fn received_bytes(store: &Store, id: &ScopeId, day: NaiveDate, service: &str) -> i64 {
	let mut tx = store
		.pool
		.begin(&TenancyScope::tenant(&id.tenant).unwrap())
		.await
		.unwrap();
	let row = ScopeDaily::objects()
		.filter(ScopeDaily::field_scope_id().eq(id.scratch_space))
		.filter(ScopeDaily::field_tenant().eq(id.tenant.clone()))
		.filter(ScopeDaily::field_node().eq(id.node.clone()))
		.filter(ScopeDaily::field_day().eq(day.to_string()))
		.filter(ScopeDaily::field_service().eq(service.to_owned()))
		.all_with_executor(&mut *tx)
		.await
		.unwrap()
		.pop()
		.unwrap();
	tx.commit().await.unwrap();
	row.received_bytes
}

struct Authority(ScopeSettings);
struct AuditMeter;
#[async_trait]
impl Meter for AuditMeter {
	async fn charge(&self, _: Direction, _: usize) -> aidash_application::Result<()> {
		Ok(())
	}
	async fn finish(
		&self,
		_: u16,
		_: RequestTimes,
		_: Option<&str>,
	) -> aidash_application::Result<()> {
		Ok(())
	}
}
/// Cannot be reached on its first charge, then commits what it is given and
/// counts it.
#[derive(Default)]
struct FlakyAuditMeter {
	calls: std::sync::atomic::AtomicU64,
	committed: std::sync::atomic::AtomicU64,
}
impl FlakyAuditMeter {
	fn committed(&self) -> u64 {
		self.committed.load(std::sync::atomic::Ordering::Relaxed)
	}
}
#[async_trait]
impl Meter for FlakyAuditMeter {
	async fn charge(&self, _: Direction, bytes: usize) -> aidash_application::Result<()> {
		use std::sync::atomic::Ordering::Relaxed;
		if self.calls.fetch_add(1, Relaxed) == 0 {
			return Err(aidash_application::Error::External(
				"audit unavailable".into(),
			));
		}
		self.committed.fetch_add(bytes as u64, Relaxed);
		Ok(())
	}
	async fn finish(
		&self,
		_: u16,
		_: RequestTimes,
		_: Option<&str>,
	) -> aidash_application::Result<()> {
		Ok(())
	}
}
/// A scope authority that cannot be asked about the scope on its first call.
struct FlakyAuthority {
	inner: Authority,
	calls: std::sync::atomic::AtomicU64,
}
#[async_trait]
impl ScopeAuthority for FlakyAuthority {
	async fn authenticate(&self, credential: &str) -> aidash_application::Result<ScopeSettings> {
		self.inner.authenticate(credential).await
	}
	async fn current(&self, id: &ScopeId) -> aidash_application::Result<ScopeSettings> {
		if self
			.calls
			.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
			== 0
		{
			return Err(aidash_application::Error::External(
				"authority unavailable".into(),
			));
		}
		self.inner.current(id).await
	}
	async fn admit_external(
		&self,
		scope: &ScopeSettings,
		service: &str,
		method: &str,
		path: &str,
		digest: &str,
	) -> aidash_application::Result<ExternalAdmission> {
		self.inner
			.admit_external(scope, service, method, path, digest)
			.await
	}
}
#[async_trait]
impl ScopeAuthority for Authority {
	async fn authenticate(&self, credential: &str) -> aidash_application::Result<ScopeSettings> {
		if credential != "credential" {
			return Err(aidash_application::Error::Unauthorized);
		}
		Ok(self.0.clone())
	}
	async fn current(&self, id: &ScopeId) -> aidash_application::Result<ScopeSettings> {
		if id != &self.0.id {
			return Err(aidash_application::Error::Forbidden);
		}
		Ok(self.0.clone())
	}
	async fn admit_external(
		&self,
		scope: &ScopeSettings,
		service: &str,
		_: &str,
		_: &str,
		_: &str,
	) -> aidash_application::Result<ExternalAdmission> {
		if scope.id != self.0.id || service != "maps" {
			return Err(aidash_application::Error::Forbidden);
		}
		Ok(ExternalAdmission::Admitted(Box::new(Admission {
			destination: aidash_domain::keyless::Destination::PublicOnly,
			request: Uuid::new_v4(),
			service: service.into(),
			definition: aidash_domain::egress::ExternalService {
				base_url: "https://maps.example/".into(),
				methods: vec!["GET".into()],
				secret: None,
				authentication: Default::default(),
				limits: Some(aidash_domain::egress::Limits {
					sent_bytes: 5,
					received_bytes: 5,
					requests: 1,
				}),
			},
			expires_at: Utc::now() + chrono::Duration::minutes(1),
			credential: None,
			route: None,
			meter: Arc::new(AuditMeter),
		})))
	}
}

fn admitted(outcome: ExternalAdmission) -> Admission {
	match outcome {
		ExternalAdmission::Admitted(admission) => *admission,
		ExternalAdmission::Refused(refusal) => panic!("refused: {refusal:?}"),
	}
}

async fn open_connection(durable: &DurableScopes, scope: &ScopeSettings) -> Uuid {
	let id = Uuid::new_v4();
	durable
		.record(ConnectionRecord {
			id,
			scope: scope.id.clone(),
			run_id: scope.run_id,
			host: "registry.npmjs.org".into(),
			port: 443,
			admission_ms: None,
			settle_ms: None,
			sent_bytes: 0,
			received_bytes: 0,
			duration_ms: 0,
			outcome: "unsettled".into(),
			error: None,
			wait_ms: None,
		})
		.await
		.unwrap();
	id
}

#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn host_debit_replays_a_cumulative_target_after_its_answer_is_lost(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	let settings = settings::settings_for(&database.url);
	let federation = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
		.await
		.unwrap();
	tenant_roots::tenant_root(&federation, "acme").await;
	let scope = ScopeSettings {
		run_id: None,
		id: ScopeId {
			tenant: "acme".into(),
			node: tenant_roots::fixture_node_name("acme").to_string(),
			scratch_space: Uuid::new_v4(),
		},
		allowed_hosts: AllowedHosts::default(),
		budget: DailyBudget {
			sent_bytes: 100,
			received_bytes: 100,
		},
		anomaly: AnomalyRule::default(),
		operator_allows_all: false,
		tunnel_limits: Default::default(),
	};
	let authority = Arc::new(Authority(scope.clone()));
	let durable = DurableScopes::new(federation.store.clone(), authority.clone());
	durable.resolve("credential").await.unwrap();
	let day = Utc::now().date_naive();
	let connection = open_connection(&durable, &scope).await;
	// Discard the committed call's answer and retry through a fresh store adapter.
	durable
		.debit(&scope, Direction::Received, 30, day, connection)
		.await
		.unwrap();
	let retry = DurableScopes::new(federation.store.clone(), authority);
	retry
		.debit(&scope, Direction::Received, 30, day, connection)
		.await
		.unwrap();
	assert_eq!(
		received_bytes(&federation.store, &scope.id, day, "").await,
		30
	);
	for target in [29, 60, 60] {
		retry
			.debit(&scope, Direction::Received, target, day, connection)
			.await
			.unwrap();
	}
	let next_day = day.succ_opt().unwrap();
	retry
		.debit(&scope, Direction::Received, 60, next_day, connection)
		.await
		.unwrap();
	retry
		.debit(&scope, Direction::Received, 62, next_day, connection)
		.await
		.unwrap();
	assert_eq!(
		received_bytes(&federation.store, &scope.id, next_day, "").await,
		2
	);
	assert_eq!(
		received_bytes(&federation.store, &scope.id, day, "").await,
		60
	);
}

/// A Tenant Budget with a 100 000 nanocredit Hard Limit, two Runs it covers,
/// and a scope credential for the first Run.
struct BudgetRig {
	_context: reinhardt::InjectionContext,
	f: crate::federation::Federation,
	run: Uuid,
	sibling: Uuid,
	space: Uuid,
	authority: Arc<ScratchAuthority>,
	durable: DurableScopes,
	credential: String,
	scope: ScopeSettings,
}

impl BudgetRig {
	async fn new(database: &DatabaseFixture, on_hard_limit: &str) -> Self {
		let context =
			reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
		let settings = settings::settings_for(&database.url);
		let f = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
			.await
			.unwrap();
		let tenant = tenant_roots::tenant_scope(&f.store, "acme").await;
		let thread = f
			.store
			.create_thread(&tenant, "alice", "Traffic budget")
			.await
			.unwrap();
		let name = aidash_domain::tenancy::TenantName::new("acme").unwrap();
		let endpoint = "http://127.0.0.1:9/v1";
		f.store.keyless.permit_origin_of(endpoint).unwrap();
		for (kind, id, config) in [
			(
				"model",
				"traffic-model",
				serde_json::json!({
					"provider": "openrouter", "model_id": "fixture", "endpoint": endpoint,
					"context_window": 128000, "max_output_tokens": 4096, "modalities": ["text"], "cost": {},
				}),
			),
			(
				"agent",
				"traffic-agent",
				serde_json::json!({
					"schema_version": 1, "model": {"id": "traffic-model", "version": "1.0.0"},
					"instructions": "Test traffic accounting", "bindings": [], "remove_default": [],
				}),
			),
		] {
			f.registry
				.register(
					&name,
					serde_json::from_value(serde_json::json!({
						"id": id, "version": "1.0.0", "kind": kind,
						"name": {"en": id}, "description": {"en": "Traffic accounting fixture"},
						"capabilities": [], "languages": ["en"], "schema": {"type": "object"},
						"config": config,
					}))
					.unwrap(),
				)
				.await
				.unwrap();
		}
		let agent = f
			.registry
			.get(&name, "traffic-agent", "1.0.0")
			.await
			.unwrap();
		let new_run = async |title: &str| {
			let task = f
				.store
				.create_task(
					&tenant,
					thread.id,
					&aidash_domain::NewTask {
						title: title.into(),
						description: "Traffic budget".into(),
						requirements: serde_json::json!({}),
						dependencies: vec![],
						parent_id: None,
					},
					"alice",
					None,
				)
				.await
				.unwrap();
			tenant_roots::admitted_run(&f, "acme", &task, &agent)
				.await
				.id
		};
		let run = new_run("Transfer").await;
		let sibling = new_run("Sibling").await;
		sqlx::query("INSERT INTO budgets (id, tenant, scope_kind, scope_key, period, hard_limit_nanocredits, on_hard_limit, recipients, revision, created_at, created_by, updated_at) VALUES ($1, 'acme', 'tenant', '', 'monthly', 100000, $2, '{}', 1, now(), 'alice', now())")
			.bind(Uuid::new_v4())
			.bind(on_hard_limit)
			.execute(f.store.pool.fixture_driver())
			.await
			.unwrap();
		let space = insert_space(&f, "acme", Default::default()).await;
		let authority = Arc::new(ScratchAuthority::with_key(
			f.store.clone(),
			f.config.scratch_policy.clone(),
			KEY,
		));
		let credential = authority.credential("acme", space, run).unwrap();
		let durable = DurableScopes::new(f.store.clone(), authority.clone());
		let scope = durable.resolve(&credential).await.unwrap();
		Self {
			_context: context,
			f,
			run,
			sibling,
			space,
			authority,
			durable,
			credential,
			scope,
		}
	}

	fn pool(&self) -> &sqlx::PgPool {
		self.f.store.pool.fixture_driver()
	}

	/// Forwards 1 MiB on a new connection, which prices past the Hard Limit.
	async fn exceed(&self) -> aidash_application::Result<()> {
		let connection = open_connection(&self.durable, &self.scope).await;
		let day = Utc::now().date_naive();
		let mib = 1 << 20;
		self.durable
			.debit(&self.scope, Direction::Sent, 2 * mib, day, connection)
			.await
			.unwrap();
		self.durable
			.report_forwarded(&self.scope, connection, Direction::Sent, mib, day)
			.await
	}

	async fn run_state(&self, run: Uuid) -> (String, Option<String>) {
		sqlx::query_as("SELECT control, error FROM task_executions WHERE id = $1")
			.bind(run)
			.fetch_one(self.pool())
			.await
			.unwrap()
	}
}

fn budget_refused<T: std::fmt::Debug>(result: &aidash_application::Result<T>) -> bool {
	matches!(result, Err(aidash_application::Error::Refused(code)) if code == "egress_budget_exhausted")
}

#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn only_forwarded_bytes_post_to_the_run_budget_and_pause_its_run(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let rig = BudgetRig::new(&database, "stop").await;
	let (durable, scope, pool) = (&rig.durable, &rig.scope, rig.pool());
	let connection = open_connection(durable, scope).await;
	let day = Utc::now().date_naive();
	let mib = 1 << 20;
	durable
		.debit(scope, Direction::Sent, 2 * mib, day, connection)
		.await
		.unwrap();
	let usage =
		|| {
			sqlx::query_as::<_, (i64, i64)>(
			"SELECT sent_bytes, cost_nanocredits FROM egress_usage_daily WHERE tenant = 'acme' AND day = $1",
		).bind(day.to_string()).fetch_optional(pool)
		};
	assert_eq!(usage().await.unwrap(), None);
	for _ in 0..2 {
		let result = durable
			.report_forwarded(scope, connection, Direction::Sent, mib, day)
			.await;
		assert!(budget_refused(&result), "{result:?}");
	}
	assert_eq!(usage().await.unwrap(), Some((mib as i64, 195_312)));
	let spend: (String, String, i64, i64) = sqlx::query_as(
		"SELECT node, root_subject, spent_nanocredits, unpriced_calls FROM budget_spend WHERE tenant = 'acme'",
	).fetch_one(pool).await.unwrap();
	assert_eq!(spend, (scope.id.node.clone(), "alice".into(), 195_312, 0));
	assert_eq!(rig.run_state(rig.run).await.0, "PAUSED");
	assert_eq!(rig.run_state(rig.sibling).await.0, "ACTIVE");
}

/// A Run's Background processes outlive it (ADR 0148), and a Run its model
/// calls paused carries their message rather than the egress one: either way
/// the Hard Limit, not the Run's state, refuses its Host traffic.
#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn a_hard_limit_refuses_host_traffic_of_finished_and_model_paused_runs(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let rig = BudgetRig::new(&database, "stop").await;
	rig.durable.admit_connection(&rig.scope).await.unwrap();
	sqlx::query(
		"UPDATE task_executions SET phase = 'COMPLETED', lease_owner = NULL, lease_until = NULL WHERE id = $1",
	)
	.bind(rig.run)
	.execute(rig.pool())
	.await
	.unwrap();
	let result = rig.exceed().await;
	assert!(budget_refused(&result), "{result:?}");
	// A finished Run is not paused, but it opens and keeps no connection.
	assert_eq!(rig.run_state(rig.run).await, ("ACTIVE".into(), None));
	let result = rig.durable.admit_connection(&rig.scope).await;
	assert!(budget_refused(&result), "{result:?}");
	let result = rig.durable.check_active(&rig.scope, &rig.credential).await;
	assert!(budget_refused(&result), "{result:?}");

	sqlx::query("UPDATE task_executions SET control = 'PAUSED', error = 'Budget is at its Hard Limit; raise it, then resume the Run' WHERE id = $1")
		.bind(rig.sibling)
		.execute(rig.pool())
		.await
		.unwrap();
	let credential = rig
		.authority
		.credential("acme", rig.space, rig.sibling)
		.unwrap();
	let sibling = rig.durable.resolve(&credential).await.unwrap();
	let result = rig.durable.admit_connection(&sibling).await;
	assert!(budget_refused(&result), "{result:?}");
	let result = rig.durable.check_active(&sibling, &credential).await;
	assert!(budget_refused(&result), "{result:?}");
}

/// A Sleep Hard Limit lets the Run wait for its Budget (ADR 0143), so egress
/// refuses traffic without pausing the Run for a person to resume.
#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn a_sleep_hard_limit_refuses_host_traffic_without_pausing_the_run(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let rig = BudgetRig::new(&database, "sleep").await;
	let result = rig.exceed().await;
	assert!(budget_refused(&result), "{result:?}");
	assert_eq!(rig.run_state(rig.run).await, ("ACTIVE".into(), None));
	let result = rig.durable.admit_connection(&rig.scope).await;
	assert!(budget_refused(&result), "{result:?}");
	let result = rig.durable.check_active(&rig.scope, &rig.credential).await;
	assert!(budget_refused(&result), "{result:?}");
}

#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn reversing_connection_accounting_restores_the_scoped_receipt_table(
	#[future] database: DatabaseFixture,
) {
	use reinhardt::db::migrations::{DatabaseMigrationExecutor, FilesystemSource, MigrationSource};

	let database = database.await;
	let source =
		FilesystemSource::new(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"));
	let migration = source
		.get_migration("execution", "0166_scope_connection_debits")
		.await
		.unwrap();
	let plugin_data_bounds = source
		.get_migration("execution", "0167_plugin_data_file_count")
		.await
		.unwrap();
	let owner = database.owner_connection.clone();
	let history = source.all_migrations().await.unwrap();
	let mut executor =
		DatabaseMigrationExecutor::new(owner.clone()).with_migration_history(history.clone());
	executor
		.rollback_migrations(&[migration, plugin_data_bounds])
		.await
		.unwrap();
	let sql = owner.into_postgres().unwrap();
	let restored: (bool, bool, i64) = sqlx::query_as(
		"SELECT relrowsecurity, relforcerowsecurity,
		        (SELECT count(*) FROM pg_constraint WHERE conrelid = pg_class.oid AND contype = 'f')
		 FROM pg_class WHERE oid = 'egress_scope_host_debits'::regclass",
	)
	.fetch_one(&sql)
	.await
	.unwrap();
	assert_eq!(restored, (true, true, 2));
	executor.apply_migrations(&history).await.unwrap();
	let retired: bool =
		sqlx::query_scalar("SELECT to_regclass('egress_scope_host_debits') IS NULL")
			.fetch_one(&sql)
			.await
			.unwrap();
	assert!(retired);
}

#[rstest::rstest]
#[case::node(false)]
#[case::tenant(true)]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn scope_purge_erases_a_daily_row_committed_while_purge_waits(
	#[future] database: DatabaseFixture,
	#[case] whole_tenant: bool,
) {
	use crate::apps::identity::services::{node_purge, tenant_purge};
	let database = database.await;
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	let settings = settings::settings_for(&database.url);
	let federation = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
		.await
		.unwrap();
	let store = &federation.store;
	let pool = store.pool.fixture_driver();
	tenant_roots::tenant_scope(store, "acme").await;
	let node = tenant_roots::second_node_in(pool, "acme").await;
	let scope = ScopeId {
		tenant: "acme".into(),
		node: node.to_string(),
		scratch_space: Uuid::new_v4(),
	};
	let day = Utc::now().date_naive();
	sqlx::query("INSERT INTO egress_scope_state (scope_id, tenant, node) VALUES ($1, 'acme', $2)")
		.bind(scope.scratch_space)
		.bind(node.as_str())
		.execute(pool)
		.await
		.unwrap();
	if whole_tenant {
		sqlx::query("INSERT INTO tenant_deletions (id, tenant, deleted_at, deleted_by_kind, purge_after, purge_step, purge_failures) VALUES ($1, 'acme', now() - interval '31 days', 'deployment_administrator', now() - interval '1 day', 'rows', 0)")
			.bind(Uuid::new_v4()).execute(pool).await.unwrap();
	} else {
		sqlx::query("INSERT INTO node_deletions (id, tenant, node, deleted_at, deleted_by_kind, purge_after, purge_step, purge_failures) VALUES ($1, 'acme', $2, now() - interval '31 days', 'deployment_administrator', now() - interval '1 day', 'rows', 0)")
			.bind(Uuid::new_v4()).bind(node.as_str()).execute(pool).await.unwrap();
	}

	// Hold the same state lock as charge and write the day's first row. Keep
	// the charge uncommitted until PostgreSQL reports purge waiting on it.
	let mut charge = begin_timed(&store.pool, "acme").await.unwrap();
	let _ = locked_state_now(&mut charge, &scope).await.unwrap();
	let mut row = daily(&mut charge, &scope, day, "").await.unwrap();
	row.received_bytes = 1;
	ScopeDaily::objects()
		.save_with_executor(&mut *charge, &row)
		.await
		.unwrap();
	let (sql, values) = Query::select()
		.expr_as(
			reinhardt::query::Expr::cust("pg_backend_pid()"),
			Alias::new("pid"),
		)
		.build(PostgresQueryBuilder);
	let pid: i32 = charge
		.fetch_one(&sql, convert_values(values))
		.await
		.unwrap()
		.get("pid")
		.unwrap();

	let purge = async {
		if whole_tenant {
			let deletion = tenant_purge::claim(store).await.unwrap().pop().unwrap();
			tenant_purge::purge_tenant(store, deletion).await.unwrap();
		} else {
			let deletion = node_purge::claim(store).await.unwrap().pop().unwrap();
			node_purge::purge_node(store, deletion).await.unwrap();
		}
	};
	let finish_charge = async {
		tokio::time::timeout(std::time::Duration::from_secs(30), async {
			while !sqlx::query_scalar::<_, bool>(
				"SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)))",
			)
			.bind(pid)
			.fetch_one(pool)
			.await
			.unwrap()
			{
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("purge must wait for the in-flight charge");
		charge.commit().await.unwrap();
	};
	tokio::join!(purge, finish_charge);
	for table in ["egress_scope_daily", "egress_scope_state"] {
		let remaining: i64 =
			sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE scope_id = $1"))
				.bind(scope.scratch_space)
				.fetch_one(pool)
				.await
				.unwrap();
		assert_eq!(remaining, 0, "{table} survives purge");
	}
}

#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn durable_scope_serializes_daily_budget_and_persists_stop_event(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	let settings = settings::settings_for(&database.url);
	let federation = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
		.await
		.unwrap();
	tenant_roots::tenant_root(&federation, "acme").await;
	tenant_roots::tenant_root(&federation, "beta").await;
	let scope = ScopeSettings {
		run_id: None,
		id: ScopeId {
			tenant: "acme".into(),
			node: tenant_roots::fixture_node_name("acme").to_string(),
			scratch_space: Uuid::new_v4(),
		},
		allowed_hosts: AllowedHosts::default(),
		budget: DailyBudget {
			sent_bytes: 10,
			received_bytes: 10,
		},
		anomaly: AnomalyRule::default(),
		operator_allows_all: false,
		tunnel_limits: Default::default(),
	};
	let durable = DurableScopes::new(federation.store.clone(), Arc::new(Authority(scope.clone())));
	assert!(durable.resolve("invalid").await.is_err());
	durable.resolve("credential").await.unwrap();
	let connection_run = Uuid::new_v4();
	let mut connection = ConnectionRecord {
		run_id: Some(connection_run),
		admission_ms: Some(5),
		settle_ms: Some(3),
		id: Uuid::new_v4(),
		scope: scope.id.clone(),
		host: "registry.npmjs.org".into(),
		port: 443,
		sent_bytes: 0,
		received_bytes: 0,
		duration_ms: 0,
		outcome: "interrupted".into(),
		error: None,
		wait_ms: None,
	};
	durable.record(connection.clone()).await.unwrap();
	connection.sent_bytes = 7;
	connection.outcome = "completed".into();
	durable.record(connection.clone()).await.unwrap();
	let recorded: (i64, i64) = sqlx::query_as("SELECT count(*), COALESCE(sum(sent_bytes), 0)::bigint FROM egress_scope_connections WHERE id = $1")
		.bind(connection.id).fetch_one(federation.store.pool.fixture_driver()).await.unwrap();
	assert_eq!(recorded, (1, 7));
	// A connection that held its limits names its Run and its timings (ADR 0136).
	let attribution: (Option<Uuid>, Option<i64>, Option<i64>) = sqlx::query_as(
		"SELECT run_id, admission_ms, settle_ms FROM egress_scope_connections WHERE id = $1",
	)
	.bind(connection.id)
	.fetch_one(federation.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(attribution, (Some(connection_run), Some(5), Some(3)));
	let invalid_target = ConnectionRecord {
		id: Uuid::new_v4(),
		port: 0,
		sent_bytes: 0,
		received_bytes: 0,
		outcome: "invalid_proxy_target".into(),
		..connection.clone()
	};
	durable.record(invalid_target).await.unwrap();
	let over_limit = ConnectionRecord {
		id: Uuid::new_v4(),
		outcome: "refused".into(),
		error: Some("egress_tenant_tunnel_limit".into()),
		wait_ms: Some(10_000),
		..connection.clone()
	};
	durable.record(over_limit.clone()).await.unwrap();
	let cause: (String, Option<String>, Option<i64>) = sqlx::query_as(
		"SELECT outcome, error, wait_ms FROM egress_scope_connections WHERE id = $1",
	)
	.bind(over_limit.id)
	.fetch_one(federation.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(
		cause,
		(
			"refused".into(),
			Some("egress_tenant_tunnel_limit".into()),
			Some(10_000)
		)
	);
	let mut other = federation
		.store
		.pool
		.begin(&TenancyScope::tenant("beta").unwrap())
		.await
		.unwrap();
	let hidden = ScopeState::objects()
		.filter(ScopeState::field_scope_id().eq(scope.id.scratch_space))
		.all_with_executor(&mut *other)
		.await
		.unwrap();
	assert!(hidden.is_empty(), "another Tenant must not see scope state");
	other.commit().await.unwrap();
	let day = Utc::now().date_naive();
	let first_connection = open_connection(&durable, &scope).await;
	let second_connection = open_connection(&durable, &scope).await;
	let (left, right) = tokio::join!(
		durable.debit(&scope, Direction::Sent, 8, day, first_connection),
		durable.debit(&scope, Direction::Sent, 8, day, second_connection)
	);
	assert_ne!(
		left.is_ok(),
		right.is_ok(),
		"exactly one concurrent debit fits"
	);
	assert!(
		durable
			.debit(
				&scope,
				Direction::Sent,
				3,
				day,
				open_connection(&durable, &scope).await
			)
			.await
			.is_err()
	);
	durable
		.refund(&scope, Direction::Sent, 1, day)
		.await
		.unwrap();
	durable
		.debit(
			&scope,
			Direction::Sent,
			2,
			day,
			open_connection(&durable, &scope).await,
		)
		.await
		.unwrap();
	let first = admitted(
		durable
			.admit_external("credential", "maps", "GET", "/", "digest")
			.await
			.unwrap(),
	);
	let second = admitted(
		durable
			.admit_external("credential", "maps", "GET", "/", "digest")
			.await
			.unwrap(),
	);
	first.meter.charge(Direction::Received, 3).await.unwrap();
	assert!(
		second
			.meter
			.charge(Direction::Received, 3)
			.await
			.unwrap_err()
			.to_string()
			.contains("egress_budget_exhausted")
	);
	let recorded = received_bytes(&federation.store, &scope.id, day, "maps").await;
	assert_eq!(
		recorded, 6,
		"already forwarded bytes remain charged after an over-limit debit"
	);
	let total = async || received_bytes(&federation.store, &scope.id, day, "").await;
	let audit = Arc::new(FlakyAuditMeter::default());
	let failed_audit =
		ScopedMeter::new(durable.clone(), scope.clone(), "maps", None, audit.clone());
	assert!(failed_audit.charge(Direction::Received, 1).await.is_err());
	assert_eq!(
		total().await,
		7,
		"audit failure must not refund forwarded bytes"
	);
	// The gateway charges the same bytes again after a failure that is not a
	// decision. The scope has them already; only the audit is still owed.
	failed_audit.charge(Direction::Received, 1).await.unwrap();
	assert_eq!(
		total().await,
		7,
		"the retry does not charge the scope twice"
	);
	assert_eq!(audit.committed(), 1, "the audit counts the bytes once");
	// The reverse: the scope could not be asked after the audit had the bytes.
	let audit = Arc::new(FlakyAuditMeter::default());
	audit.calls.store(1, std::sync::atomic::Ordering::Relaxed);
	let unreachable = ScopedMeter::new(
		DurableScopes::new(
			federation.store.clone(),
			Arc::new(FlakyAuthority {
				inner: Authority(scope.clone()),
				calls: Default::default(),
			}),
		),
		scope.clone(),
		"maps",
		None,
		audit.clone(),
	);
	assert!(unreachable.charge(Direction::Received, 1).await.is_err());
	assert_eq!((total().await, audit.committed()), (7, 1));
	unreachable.charge(Direction::Received, 1).await.unwrap();
	assert_eq!(
		(total().await, audit.committed()),
		(8, 1),
		"the retry charges the scope and not the audit again"
	);
	// What both sides acknowledged is not carried over to the next charge.
	unreachable.charge(Direction::Received, 1).await.unwrap();
	assert_eq!((total().await, audit.committed()), (9, 2));
	// A spent service share refuses the next request in the authority, before
	// it writes a request record: see
	// `a_spent_budget_refuses_an_external_service_request_before_its_record`.
	// No number of connections stops a scope (#845).
	for _ in 0..3 {
		durable.admit_connection(&scope).await.unwrap();
	}
	durable.check_active(&scope, "credential").await.unwrap();
	let minute = chrono::DateTime::from_timestamp(Utc::now().timestamp() / 60 * 60, 0).unwrap();
	durable
		.record_refusals(vec![
			aidash_application::egress::RefusalTally {
				scope: scope.id.clone(),
				minute,
				host: "denied.example".into(),
				reason: "egress_host_forbidden".into(),
				count: 3,
			},
			aidash_application::egress::RefusalTally {
				scope: scope.id.clone(),
				minute,
				host: String::new(),
				reason: "egress_client_closed".into(),
				count: 2,
			},
		])
		.await
		.unwrap();
	let refusals: Vec<(String, String, i64)> = sqlx::query_as(
		"SELECT host, reason, count FROM egress_scope_refusals WHERE scope_id = $1 ORDER BY reason",
	)
	.bind(scope.id.scratch_space)
	.fetch_all(federation.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(
		refusals,
		vec![
			(String::new(), "egress_client_closed".into(), 2),
			("denied.example".into(), "egress_host_forbidden".into(), 3),
		]
	);
	// A Tenant whose tallies cannot be written (here one that does not exist,
	// written first) does not lose another Tenant's counts.
	let gone = ScopeId {
		tenant: "aaa-gone".into(),
		node: "gone".into(),
		scratch_space: Uuid::new_v4(),
	};
	assert!(
		durable
			.record_refusals(vec![
				aidash_application::egress::RefusalTally {
					scope: gone,
					minute,
					host: String::new(),
					reason: "egress_client_closed".into(),
					count: 1,
				},
				aidash_application::egress::RefusalTally {
					scope: scope.id.clone(),
					minute,
					host: "later.example".into(),
					reason: "egress_host_forbidden".into(),
					count: 4,
				},
			])
			.await
			.is_err(),
		"the failed Tenant is still reported"
	);
	let later: i64 = sqlx::query_scalar(
		"SELECT count FROM egress_scope_refusals WHERE scope_id = $1 AND host = 'later.example'",
	)
	.bind(scope.id.scratch_space)
	.fetch_one(federation.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(later, 4);
	durable.notify_many_hosts(&scope.id, 101).await.unwrap();
	let events: i64 = sqlx::query_scalar(
		"SELECT count(*) FROM egress_scope_events WHERE scope_id = $1 AND kind = 'many_hosts'",
	)
	.bind(scope.id.scratch_space)
	.fetch_one(federation.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(events, 1);
	durable
		.debit(
			&scope,
			Direction::Sent,
			1,
			day,
			open_connection(&durable, &scope).await,
		)
		.await
		.unwrap();
	for _ in 0..2 {
		assert!(
			durable
				.admit_connection(&scope)
				.await
				.unwrap_err()
				.to_string()
				.contains("egress_budget_exhausted")
		);
	}
}

#[test]
fn storage_counter_ceiling_cannot_turn_a_budget_into_unlimited_traffic() {
	let row = ScopeDaily::build()
		.id(Uuid::new_v4())
		.tenant("acme".to_owned())
		.node("node".to_owned())
		.scope_id(Uuid::new_v4())
		.day("2026-01-01".to_owned())
		.service(String::new())
		.sent_bytes(i64::MAX)
		.received_bytes(0)
		.finish();
	assert!(budget_available(&row, u64::MAX, u64::MAX).is_err());
}

/// Admission takes no state lock (#845), so a burst of a Space's first
/// connections of a day must not race each other to create the day's counter.
#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn concurrent_first_admissions_of_a_day_write_no_counter_and_all_pass(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	let settings = settings::settings_for(&database.url);
	let federation = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
		.await
		.unwrap();
	tenant_roots::tenant_root(&federation, "acme").await;
	let scope = ScopeSettings {
		run_id: None,
		id: ScopeId {
			tenant: "acme".into(),
			node: tenant_roots::fixture_node_name("acme").to_string(),
			scratch_space: Uuid::new_v4(),
		},
		allowed_hosts: AllowedHosts::default(),
		budget: DailyBudget {
			sent_bytes: 10,
			received_bytes: 10,
		},
		anomaly: AnomalyRule::default(),
		operator_allows_all: false,
		tunnel_limits: Default::default(),
	};
	let durable = DurableScopes::new(federation.store.clone(), Arc::new(Authority(scope.clone())));
	durable.resolve("credential").await.unwrap();
	let admissions =
		futures_util::future::join_all((0..16).map(|_| durable.admit_connection(&scope)));
	for admitted in admissions.await {
		admitted.unwrap();
	}
	let rows: (i64,) =
		sqlx::query_as("SELECT count(*) FROM egress_scope_daily WHERE scope_id = $1")
			.bind(scope.id.scratch_space)
			.fetch_one(federation.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(rows.0, 0, "admission only reads the day's counter");
	// A day with nothing charged still counts against a zero budget.
	let mut empty = scope.clone();
	empty.budget = DailyBudget {
		sent_bytes: 0,
		received_bytes: 10,
	};
	let durable = DurableScopes::new(federation.store.clone(), Arc::new(Authority(empty.clone())));
	assert!(
		durable
			.admit_connection(&empty)
			.await
			.unwrap_err()
			.to_string()
			.contains("egress_budget_exhausted")
	);
}

const KEY: &str = "scope-test-key";

#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn initialization_cache_is_bounded_and_evicted_scopes_keep_their_budget(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	let settings = settings::settings_for(&database.url);
	let federation = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
		.await
		.unwrap();
	tenant_roots::tenant_root(&federation, "acme").await;
	let space = insert_space(&federation, "acme", Default::default()).await;
	let authority = Arc::new(ScratchAuthority::with_key(
		federation.store.clone(),
		federation.config.scratch_policy.clone(),
		KEY,
	));
	let credential = authority.credential("acme", space, Uuid::new_v4()).unwrap();
	let durable = DurableScopes::new(federation.store.clone(), authority);
	let scope = durable.resolve(&credential).await.unwrap();
	let day = Utc::now().date_naive();
	durable
		.debit(
			&scope,
			Direction::Received,
			5,
			day,
			open_connection(&durable, &scope).await,
		)
		.await
		.unwrap();

	for _ in 0..INITIALIZED_KEPT {
		let other = ScopeId {
			scratch_space: Uuid::new_v4(),
			..scope.id.clone()
		};
		durable.initialize(&other).await.unwrap();
	}
	{
		let initialized = durable.initialized.lock().unwrap();
		assert!(initialized.len() <= INITIALIZED_KEPT);
		assert!(!initialized.contains(&scope.id));
	}

	let reopened = durable.resolve(&credential).await.unwrap();
	durable
		.debit(
			&reopened,
			Direction::Received,
			7,
			day,
			open_connection(&durable, &reopened).await,
		)
		.await
		.unwrap();
	assert_eq!(
		received_bytes(&federation.store, &scope.id, day, "").await,
		12
	);
}

async fn insert_space(
	federation: &crate::federation::Federation,
	tenant: &str,
	settings: aidash_domain::scratch::ScratchSettings,
) -> Uuid {
	let now = Utc::now();
	let space = aidash_domain::scratch::ScratchSpace {
		id: Uuid::new_v4(),
		node: tenant_roots::fixture_node_name(tenant),
		name: "scope".to_owned(),
		creator: "owner".to_owned(),
		state: aidash_domain::scratch::ScratchState::Active,
		revision: 0,
		wipe_generation: 0,
		settings,
		members: Default::default(),
		created_at: now,
		last_used_at: now,
	};
	let mut tx = federation
		.store
		.pool
		.begin(&TenancyScope::tenant(tenant).unwrap())
		.await
		.unwrap();
	crate::apps::workspaces::repositories::scratch::insert(&mut *tx, tenant, &space)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	space.id
}

async fn delete_space(federation: &crate::federation::Federation, tenant: &str, space: Uuid) {
	let mut tx = federation
		.store
		.pool
		.begin(&TenancyScope::tenant(tenant).unwrap())
		.await
		.unwrap();
	crate::apps::workspaces::repositories::scratch::delete(&mut *tx, tenant, space)
		.await
		.unwrap();
	tx.commit().await.unwrap();
}

#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn a_scope_credential_opens_its_live_scratch_space_until_the_space_is_deleted(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	let settings = settings::settings_for(&database.url);
	let federation = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
		.await
		.unwrap();
	tenant_roots::tenant_root(&federation, "acme").await;
	let mut chosen = aidash_domain::scratch::ScratchSettings {
		daily_sent_bytes: Some(11),
		daily_received_bytes: Some(22),
		..Default::default()
	};
	chosen.allowed_hosts = aidash_domain::scratch::HostAllowlist::Hosts(
		["registry.npmjs.org", "not-allowed-by-the-tenant.example"]
			.map(str::to_owned)
			.into(),
	);
	let space = insert_space(&federation, "acme", chosen).await;
	let policy = federation.config.scratch_policy.clone();
	let authority = Arc::new(ScratchAuthority::with_key(
		federation.store.clone(),
		policy,
		KEY,
	));
	let run_id = Uuid::new_v4();
	let credential = authority.credential("acme", space, run_id).unwrap();
	let durable = DurableScopes::new(federation.store.clone(), authority.clone());

	let resolved = durable.resolve(&credential).await.unwrap();
	assert_eq!(resolved.run_id, Some(run_id));
	assert_eq!(resolved.id.tenant, "acme");
	assert_eq!(resolved.id.scratch_space, space);
	assert_eq!(
		resolved.id.node,
		tenant_roots::fixture_node_name("acme").to_string()
	);
	assert_eq!(resolved.budget.sent_bytes, 11);
	assert_eq!(resolved.budget.received_bytes, 22);
	assert_eq!(
		resolved.allowed_hosts.hosts,
		vec!["registry.npmjs.org".to_owned()],
		"the Tenant's limit narrows the space's own hosts"
	);
	durable.admit_connection(&resolved).await.unwrap();
	durable.check_active(&resolved, &credential).await.unwrap();

	let foreign = ScratchAuthority::with_key(
		federation.store.clone(),
		federation.config.scratch_policy.clone(),
		"another-key",
	)
	.credential("acme", space, Uuid::new_v4())
	.unwrap();
	assert!(matches!(
		durable.resolve(&foreign).await,
		Err(aidash_application::Error::Unauthorized)
	));
	// The Scratch Space has no External services, so it names none.
	assert!(matches!(
		durable
			.admit_external(&credential, "maps", "GET", "/", &"0".repeat(64))
			.await,
		Ok(ExternalAdmission::Refused(aidash_application::egress::ServiceRefusal { code, available }))
			if code == aidash_domain::egress::SERVICE_NOT_AVAILABLE && available.is_empty()
	));

	let row = RunRevocation::build()
		.tenant("acme".to_owned())
		.run_id(run_id)
		.revoked_at(Utc::now())
		.reason("read_refused".to_owned())
		.stopped_at(None)
		.finish();
	let mut tx = federation
		.store
		.pool
		.begin(&TenancyScope::tenant("acme").unwrap())
		.await
		.unwrap();
	RunRevocation::objects()
		.insert_with_executor(&mut *tx, &row)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	assert!(matches!(
		durable.resolve(&credential).await,
		Err(aidash_application::Error::Forbidden)
	));
	assert!(matches!(
		durable.admit_connection(&resolved).await,
		Err(aidash_application::Error::Forbidden)
	));
	assert!(matches!(
		durable.check_active(&resolved, &credential).await,
		Err(aidash_application::Error::Forbidden)
	));
	let other = authority.credential("acme", space, Uuid::new_v4()).unwrap();
	assert!(durable.resolve(&other).await.is_ok());
	let preparation = authority
		.preparation_credential("acme", space, run_id)
		.unwrap();
	assert_eq!(durable.resolve(&preparation).await.unwrap().run_id, None);

	delete_space(&federation, "acme", space).await;
	assert!(matches!(
		durable.resolve(&other).await,
		Err(aidash_application::Error::Unauthorized)
	));
}

/// An External service request refused at admission, for a spent daily share
/// or for limits above the Deployment's current maximums, is refused before
/// its request record is written, so the Run lists no unfinished request (#738).
#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn a_spent_budget_refuses_an_external_service_request_before_its_record(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	let settings = settings::settings_for(&database.url);
	let federation = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
		.await
		.unwrap();
	tenant_roots::tenant_scope(&federation.store, "acme").await;
	federation
		.registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			serde_json::from_value(serde_json::json!({
				"id": "maps", "version": "1.0.0", "kind": "external_service",
				"name": {"en": "maps"}, "description": {"en": "Fixture service"},
				"config": {
					"base_url": "https://maps.example.test/", "methods": ["GET"],
					"limits": {"sent_bytes": 5, "received_bytes": 5, "requests": 10},
				},
			}))
			.unwrap(),
		)
		.await
		.unwrap();
	let node = tenant_roots::fixture_node_name("acme");
	crate::egress_tests::set_approval(&federation, &node, "maps", "1.0.0", true).await;
	let space = insert_space(
		&federation,
		"acme",
		aidash_domain::scratch::ScratchSettings {
			daily_sent_bytes: Some(100),
			daily_received_bytes: Some(100),
			external_services: ["maps".to_owned()].into(),
			..Default::default()
		},
	)
	.await;
	let durable_for = |store: Store| {
		let authority = Arc::new(ScratchAuthority::with_key(
			store.clone(),
			federation.config.scratch_policy.clone(),
			KEY,
		));
		(authority.clone(), DurableScopes::new(store, authority))
	};
	let (authority, durable) = durable_for(federation.store.clone());
	let run = Uuid::new_v4();
	let credential = authority.credential("acme", space, run).unwrap();
	let digest = "0".repeat(64);
	let pool = federation.store.pool.fixture_driver();
	// Every request record of the Run, and those never finished.
	let records = async || {
		sqlx::query_as::<_, (i64, i64)>(
			"SELECT count(*), count(*) FILTER (WHERE status = 0) FROM egress_requests WHERE run_id = $1",
		)
		.bind(run)
		.fetch_one(pool)
		.await
		.unwrap()
	};

	// The Deployment lowered its maximums below the service's limits after
	// the version was registered: the version is not used above them.
	let mut profile = (*federation.store.capabilities.0).clone();
	profile.runner = Some(
		serde_json::from_value(serde_json::json!({
			"endpoint": "http://127.0.0.1:9", "token_env": "AIDASH_SCOPE_EGRESS_TEST_TOKEN",
			"image": format!("fixture@sha256:{}", "b".repeat(64)),
			"runtime_class": "fixture", "namespace": "fixture",
		}))
		.unwrap(),
	);
	profile.egress = Some(
		serde_json::from_value(serde_json::json!({
			"listen": "127.0.0.1:0",
			"limits": {"sent_bytes": 4, "received_bytes": 4, "requests": 10},
		}))
		.unwrap(),
	);
	let mut lowered = federation.store.clone();
	lowered.capabilities = crate::capabilities::Runtime::new(profile).unwrap();
	let (_, lowered) = durable_for(lowered);
	assert!(matches!(
		lowered
			.admit_external(&credential, "maps", "GET", "/", &digest)
			.await,
		Err(aidash_application::Error::Invalid(message))
			if message.contains("Deployment ceilings")
	));
	assert_eq!(records().await, (0, 0));

	// Within its share the request is admitted and recorded, and it spends
	// the share.
	let admission = admitted(
		durable
			.admit_external(&credential, "maps", "GET", "/", &digest)
			.await
			.unwrap(),
	);
	assert!(matches!(
		admission.meter.charge_cumulative(Direction::Received, 5, 5).await,
		Err(aidash_application::Error::Refused(code)) if code == "egress_budget_exhausted"
	));
	assert!(matches!(
		admission.meter.charge_cumulative(Direction::Received, 4, 4).await,
		Err(aidash_application::Error::Refused(code)) if code == "egress_budget_exhausted"
	));
	let received: i64 =
		sqlx::query_scalar("SELECT received_bytes FROM egress_requests WHERE id = $1")
			.bind(admission.request)
			.fetch_one(pool)
			.await
			.unwrap();
	assert_eq!(received, 5);
	let scope = durable.resolve(&credential).await.unwrap();
	assert_eq!(
		received_bytes(
			&federation.store,
			&scope.id,
			Utc::now().date_naive(),
			"maps"
		)
		.await,
		5
	);

	admission
		.meter
		.finish(200, RequestTimes::default(), None)
		.await
		.unwrap();
	assert_eq!(records().await, (1, 0));

	// The gateway answers 429 to this refusal, and the retries are refused
	// too. None of them leaves a record behind.
	for _ in 0..2 {
		assert!(matches!(
			durable
				.admit_external(&credential, "maps", "GET", "/", &digest)
				.await,
			Err(aidash_application::Error::Refused(code)) if code == "egress_budget_exhausted"
		));
	}
	assert_eq!(
		records().await,
		(1, 0),
		"a request refused at admission leaves no unfinished request record"
	);
}

/// A deletion is recorded for the Tenant or the Node and leaves the Scratch
/// Space Active until its data is purged, so the credential has to look.
#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn deleting_the_tenant_or_the_node_closes_the_scope_credential_until_it_is_restored(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	let settings = settings::settings_for(&database.url);
	let federation = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
		.await
		.unwrap();
	tenant_roots::tenant_root(&federation, "acme").await;
	let space = insert_space(&federation, "acme", Default::default()).await;
	let authority = Arc::new(ScratchAuthority::with_key(
		federation.store.clone(),
		federation.config.scratch_policy.clone(),
		KEY,
	));
	let credential = authority.credential("acme", space, Uuid::new_v4()).unwrap();
	let durable = DurableScopes::new(federation.store.clone(), authority);
	let open = durable.resolve(&credential).await.unwrap();
	let node = tenant_roots::fixture_node_name("acme").to_string();
	let pool = federation.store.pool.fixture_driver();
	let closed = async || {
		// Admission, the periodic recheck and the live read behind a debit.
		assert!(matches!(
			durable.resolve(&credential).await,
			Err(aidash_application::Error::Forbidden)
		));
		assert!(matches!(
			durable.check_active(&open, &credential).await,
			Err(aidash_application::Error::Forbidden)
		));
		assert!(matches!(
			durable.admit_connection(&open).await,
			Err(aidash_application::Error::Forbidden)
		));
	};

	sqlx::query(
		"INSERT INTO tenant_deletions \
		 (id, tenant, deleted_at, deleted_by_kind, purge_after, purge_failures) \
		 VALUES ($1, 'acme', now(), 'deployment_administrator', now() + interval '30 days', 0)",
	)
	.bind(Uuid::new_v4())
	.execute(pool)
	.await
	.unwrap();
	closed().await;
	sqlx::query(
		"UPDATE tenant_deletions SET restored_at = now(), \
		 restored_by_kind = 'deployment_administrator' WHERE tenant = 'acme'",
	)
	.execute(pool)
	.await
	.unwrap();
	durable.resolve(&credential).await.unwrap();
	durable.check_active(&open, &credential).await.unwrap();

	sqlx::query(
		"INSERT INTO node_deletions \
		 (id, tenant, node, deleted_at, deleted_by_kind, purge_after, purge_failures) \
		 VALUES ($1, 'acme', $2, now(), 'deployment_administrator', now() + interval '30 days', 0)",
	)
	.bind(Uuid::new_v4())
	.bind(&node)
	.execute(pool)
	.await
	.unwrap();
	closed().await;
	sqlx::query(
		"UPDATE node_deletions SET restored_at = now(), \
		 restored_by_kind = 'deployment_administrator' WHERE tenant = 'acme' AND node = $1",
	)
	.bind(&node)
	.execute(pool)
	.await
	.unwrap();
	durable.resolve(&credential).await.unwrap();
}

/// Bytes forwarded while the Tenant is deleted still count against the day's
/// budget once it is restored (#843).
#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn forwarded_bytes_count_for_the_scope_while_its_authority_is_refused(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	let settings = settings::settings_for(&database.url);
	let federation = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
		.await
		.unwrap();
	tenant_roots::tenant_root(&federation, "acme").await;
	let chosen = aidash_domain::scratch::ScratchSettings {
		daily_sent_bytes: Some(10),
		daily_received_bytes: Some(10),
		..Default::default()
	};
	let space = insert_space(&federation, "acme", chosen).await;
	let authority = Arc::new(ScratchAuthority::with_key(
		federation.store.clone(),
		federation.config.scratch_policy.clone(),
		KEY,
	));
	let credential = authority.credential("acme", space, Uuid::new_v4()).unwrap();
	let durable = DurableScopes::new(federation.store.clone(), authority);
	let open = durable.resolve(&credential).await.unwrap();
	let day = Utc::now().date_naive();
	let pool = federation.store.pool.fixture_driver();
	let audit = Arc::new(FlakyAuditMeter::default());
	let meter = ScopedMeter::new(durable.clone(), open.clone(), "maps", None, audit.clone());

	sqlx::query(
		"INSERT INTO tenant_deletions \
		 (id, tenant, deleted_at, deleted_by_kind, purge_after, purge_failures) \
		 VALUES ($1, 'acme', now(), 'deployment_administrator', now() + interval '30 days', 0)",
	)
	.bind(Uuid::new_v4())
	.execute(pool)
	.await
	.unwrap();
	// The audit side is unreachable at first, so the gateway charges again;
	// the scope side has already taken its bytes and does not take them twice.
	assert!(matches!(
		meter.charge(Direction::Received, 10).await,
		Err(aidash_application::Error::External(_))
	));
	assert!(matches!(
		meter.charge(Direction::Received, 10).await,
		Err(aidash_application::Error::Forbidden)
	));
	assert_eq!(
		(
			received_bytes(&federation.store, &open.id, day, "").await,
			audit.committed()
		),
		(10, 10),
		"both meters keep bytes forwarded after the refusal"
	);

	sqlx::query(
		"UPDATE tenant_deletions SET restored_at = now(), \
		 restored_by_kind = 'deployment_administrator' WHERE tenant = 'acme'",
	)
	.execute(pool)
	.await
	.unwrap();
	assert!(matches!(
		durable.debit(&open, Direction::Received, 1, day, open_connection(&durable, &open).await).await,
		Err(aidash_application::Error::Refused(code)) if code == "egress_budget_exhausted"
	));

	// Delete the authority again and clear prior daily rows before testing the
	// refused path with no state.
	sqlx::query(
		"UPDATE tenant_deletions SET restored_at = NULL, restored_by_kind = NULL WHERE tenant = 'acme'",
	)
	.execute(pool)
	.await
	.unwrap();
	sqlx::query("DELETE FROM egress_scope_daily WHERE scope_id = $1 AND day = $2")
		.bind(space)
		.bind(day.to_string())
		.execute(pool)
		.await
		.unwrap();
	// A scope whose state is gone keeps nothing and still refuses.
	sqlx::query("DELETE FROM egress_scope_state WHERE scope_id = $1")
		.bind(space)
		.execute(pool)
		.await
		.unwrap();
	assert!(matches!(
		meter.charge(Direction::Sent, 1).await,
		Err(aidash_application::Error::Forbidden)
	));
	let rows: i64 = sqlx::query_scalar(
		"SELECT count(*) FROM egress_scope_daily WHERE scope_id = $1 AND day = $2",
	)
	.bind(space)
	.bind(day.to_string())
	.fetch_one(pool)
	.await
	.unwrap();
	assert_eq!(rows, 0);
}

/// Reads until the gateway closes the connection.
async fn proxy_request(address: std::net::SocketAddr, request: String) -> String {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
	client.write_all(request.as_bytes()).await.unwrap();
	let mut response = String::new();
	client.read_to_string(&mut response).await.unwrap();
	response
}

fn connect_request(target: &str, credential: &str) -> String {
	format!(
		"CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Bearer {credential}\r\nConnection: close\r\n\r\n"
	)
}

#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn a_real_listener_records_host_connections_in_the_database(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	let settings = settings::settings_for(&database.url);
	let federation = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
		.await
		.unwrap();
	tenant_roots::tenant_root(&federation, "acme").await;
	let space = insert_space(&federation, "acme", Default::default()).await;
	let authority = Arc::new(ScratchAuthority::with_key(
		federation.store.clone(),
		federation.config.scratch_policy.clone(),
		KEY,
	));
	let run_id = Uuid::new_v4();
	let credential = authority.credential("acme", space, run_id).unwrap();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let address = listener.local_addr().unwrap();
	let (stop, receiver) = tokio::sync::watch::channel(false);
	let gateway = tokio::spawn(aidash_integrations::egress::serve_with_scopes(
		listener,
		Arc::new(DurableScopes::new(federation.store.clone(), authority)),
		Default::default(),
		receiver,
	));

	let refused = proxy_request(address, connect_request("registry.npmjs.org:443", "wrong")).await;
	assert!(refused.contains("proxy_auth_required"), "{refused}");
	let forbidden = proxy_request(
		address,
		connect_request("not-allowed.example:443", &credential),
	)
	.await;
	assert!(forbidden.contains("egress_host_forbidden"), "{forbidden}");
	// Port 0 and a port other than 443 are both invalid targets.
	for port in [22, 0] {
		let invalid = proxy_request(
			address,
			connect_request(&format!("registry.npmjs.org:{port}"), &credential),
		)
		.await;
		assert!(invalid.contains("invalid_proxy_target"), "{invalid}");
	}
	stop.send(true).unwrap();
	gateway.await.unwrap().unwrap();

	// Each ended before it held a Host connection limit: no row, a tally.
	let rows: i64 =
		sqlx::query_scalar("SELECT count(*) FROM egress_scope_connections WHERE scope_id = $1")
			.bind(space)
			.fetch_one(federation.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(rows, 0);
	let tallies: Vec<(String, String, i64)> = sqlx::query_as(
		"SELECT host, reason, sum(count)::bigint FROM egress_scope_refusals WHERE scope_id = $1 GROUP BY 1, 2 ORDER BY 1",
	)
	.bind(space)
	.fetch_all(federation.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(
		tallies,
		vec![
			(
				"not-allowed.example".to_owned(),
				"egress_host_forbidden".to_owned(),
				1
			),
			(
				"registry.npmjs.org".to_owned(),
				"invalid_proxy_target".to_owned(),
				2
			),
		]
	);
}
