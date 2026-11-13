//! The real gateway authority and meters share PostgreSQL budgets across calls.
#[path = "../../tests/support/settings.rs"]
pub(crate) mod settings;
#[path = "../../tests/support/statements.rs"]
pub(crate) mod statements;
#[path = "../../tests/support/tenant_roots.rs"]
pub(crate) mod tenant_roots;
use crate::apps::execution::test_database::{DatabaseFixture, database};

use crate::apps::execution::{
	capabilities::{
		models::{
			CoreOperations,
			egress::{Budget, RequestRecord},
		},
		services::sessions,
	},
	repositories::egress::{Gateway, attach},
};
use crate::{
	apps::identity::models::AuthorizationBundle,
	authorization::{Authorization, access::Access, identity::Actor},
	capabilities::{Profile, RunnerProfile, Runtime},
};
use aidash_application::{
	egress::{Authority, Direction},
	tenancy::{TEST_FIXTURE, TenancyScope},
};
use chrono::Utc;
use reinhardt::{
	Response, ServerRouter,
	db::{
		backends::TransactionExecutor,
		orm::{Json, Model, execution::convert_values},
	},
	query::{Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder},
};
use serde_json::json;
use uuid::Uuid;

const RUNNER_TOKEN_ENV: &str = "AIDASH_TEST_EGRESS_RUNNER_TOKEN";

#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn native_gateway_serializes_cumulative_limits_and_refuses_revoked_bindings(
	#[future] database: DatabaseFixture,
) {
	if std::env::var_os(RUNNER_TOKEN_ENV).is_none() {
		// Give only this fixture process a Runner credential without mutating the
		// environment shared by other tests or requiring manual credential setup.
		let status = tokio::process::Command::new(std::env::current_exe().unwrap())
			.args([
				"--exact",
				"egress_tests::native_gateway_serializes_cumulative_limits_and_refuses_revoked_bindings",
				"--nocapture",
			])
			.env(RUNNER_TOKEN_ENV, "native-egress-fixture-token")
			.kill_on_drop(true)
			.status()
			.await
			.unwrap();
		assert!(status.success(), "native gateway fixture failed: {status}");
		return;
	}
	let database = database.await;
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	let settings = settings::settings_for(&database.url);
	let mut f = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
		.await
		.unwrap();
	let started = Utc::now().timestamp_micros() as f64 / 1_000_000.0;
	let runner = reinhardt::test::fixtures::server::test_server_guard(
		ServerRouter::new().handler("/{*rest}", RunnerReply { started }),
	)
	.await;
	let directory = tempfile::tempdir().unwrap();
	let limits = aidash_domain::egress::Limits {
		sent_bytes: 10,
		received_bytes: 12,
		requests: 2,
	};
	let mut profile = Profile {
		admission: true,
		storage: directory.path().to_owned(),
		..Profile::default()
	};
	profile.runner = Some(RunnerProfile {
		endpoint: runner.url.clone(),
		token_env: RUNNER_TOKEN_ENV.into(),
		image: format!("sandbox@sha256:{}", "b".repeat(64)),
		runtime_class: "fixture-runsc".into(),
		namespace: "fixture-sandbox".into(),
		node_guard: None,
		journal: None,
		kubectl: None,
		kubeconfig: None,
		listen_host: None,
		listen_port: None,
	});
	profile.egress = Some(
		crate::apps::execution::capabilities::serializers::core::EgressSettings {
			endpoint: "http://10.96.0.90:8950".into(),
			namespace: None,
			pod_selector: Default::default(),
			listen: "127.0.0.1:0".parse().unwrap(),
			proxy_global_tunnels: 1024,
			proxy_tenant_tunnels: 256,
			limits,
		},
	);
	f.store.capabilities = Runtime::new(profile).unwrap();
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let scope = tenant_roots::tenant_scope(&f.store, "acme").await;
	let encrypted = std::sync::Arc::new(
		crate::apps::identity::repositories::secret_store::PostgresStore::new(
			f.store.control_pool.clone(),
			"11".repeat(32).into(),
			vec![],
		)
		.await
		.unwrap(),
	);
	let secrets = std::sync::Arc::new(aidash_application::tenant_secrets::Service {
		repository: std::sync::Arc::new(
			crate::apps::identity::repositories::tenant_secrets::NativeRepository {
				pool: f.store.control_pool.clone(),
			},
		),
		store: encrypted.clone(),
		reader: Some(encrypted),
		calls: None,
		fingerprint_key: "tenant-secret-fingerprint-fixture-32-bytes".into(),
		max_per_tenant: 64,
	});
	secrets
		.put(
			"acme",
			"image-key",
			aidash_application::tenant_secrets::Put {
				expected_revision: 0,
				value: Some("live-preview-secret-canary".into()),
				hosts: Some(vec!["api.example.test".into()]),
			},
			"alice",
		)
		.await
		.unwrap();
	f.store.tenant_secrets = Some(secrets);
	f.registry = f.registry.clone().with_tenant_secrets(true);
	let service = serde_json::from_value(
		json!({"id":"images","version":"1.0.0","kind":"external_service",
        "name":{"en":"Images"},"description":{"en":"Fixture service"},
        "config":{"base_url":"https://api.example.test/","methods":["GET","POST"]}}),
	)
	.unwrap();
	f.registry.register(&tenant, service).await.unwrap();
	f.registry
		.register(
			&tenant,
			serde_json::from_value(json!({
				"id":"secured-images","version":"1.0.0","kind":"external_service",
				"name":{"en":"Secured images"},"description":{"en":"Secret-bound fixture service"},
				"config":{"base_url":"https://api.example.test/","methods":["GET"],"secret":"image-key"}
			}))
			.unwrap(),
		)
		.await
		.unwrap();
	for (kind, id, config) in [
		(
			"model",
			"model",
			json!({"provider":"openrouter","model_id":"fixture","endpoint":"https://model.example.test/v1","context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}),
		),
		(
			"agent",
			"research",
			json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Fixture",
            "bindings":[{"kind":"external_service","target":{"registry_deployment":f.store.deployment.to_string(),"id":"images","version":"1.0.0"}}, {"kind":"external_service","target":{"registry_deployment":f.store.deployment.to_string(),"id":"secured-images","version":"1.0.0"}}],"remove_default":[]}),
		),
	] {
		f.registry.register(&tenant, serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"Fixture"},"config":config})).unwrap()).await.unwrap();
	}
	let agent = f.registry.get(&tenant, "research", "1.0.0").await.unwrap();
	let workspace = f
		.store
		.create_workspace(&scope, "alice", "Gateway", "Cumulative limits")
		.await
		.unwrap();
	let task = f
		.store
		.create_task(
			&scope,
			workspace.id,
			&serde_json::from_value(json!({"title":"Gateway","description":"Cumulative limits"}))
				.unwrap(),
			"alice",
			None,
		)
		.await
		.unwrap();
	let run = tenant_roots::admitted_run(&f, "acme", &task, &agent).await;
	let auth = Authorization {
		pool: f.store.pool.clone(),
	};
	let token = auth
		.issue_credential("acme", "alice", 3600, "deployment_administrator")
		.await
		.unwrap()
		.token;
	let Actor::Subject(identity) = auth.authenticate(&token).await.unwrap() else {
		panic!("subject")
	};
	let mut access = Access::begin(&f.store, &identity).await.unwrap();
	access.subjects.push(aidash_domain::qualified_agent(
		&f.store.deployment.node(run.node.clone()),
		&agent.id,
		&agent.version,
	));
	let area = sessions::for_run(&mut access, &run).await.unwrap();
	crate::apps::execution::repositories::operations::set_area(
		&mut access,
		area.id,
		"running",
		area.epoch,
	)
	.await
	.unwrap();
	// A retained context remains usable while a command holds the Area busy.
	sessions::context_authority(&mut access, run.metadata())
		.await
		.unwrap();
	let operation = Uuid::now_v7();
	let row = CoreOperations::build()
		.id(operation)
		.area_id(area.id)
		.run_id(run.id)
		.credential_id(identity.credential_id)
		.tenant("acme".to_owned())
		.principal(identity.subject.clone())
		.request_key("egress-fixture".to_owned())
		.digest("fixture".to_owned())
		.kind("shell".to_owned())
		.state("running".to_owned())
		.epoch(area.epoch)
		.generation(area.generation)
		.revision(area.revision)
		.policy_revision(access.snapshot.revision)
		.subjects(Json(serde_json::to_value(&access.subjects).unwrap()))
		.input(Json(json!({"seconds":120})))
		.result(Json(json!({})))
		.runner_instance(None)
		.finish();
	CoreOperations::objects()
		.insert_with_executor(&mut **access.tx, &row)
		.await
		.unwrap();
	let mut request = json!({"operation_id":operation,"seconds":120});
	attach(&f.store, &mut access, &mut request).await.unwrap();
	access.finish(Ok(())).await.unwrap();
	let db = f.store.pool.fixture_driver();
	sqlx::query("UPDATE core_operations SET state = 'submitted' WHERE id = $1")
		.bind(operation)
		.execute(db)
		.await
		.unwrap();
	let count: i64 = sqlx::query_scalar(
		"SELECT count(*) FROM run_activations WHERE run_id = $1 AND reason = 'operation_finished'",
	)
	.bind(run.id)
	.fetch_one(db)
	.await
	.unwrap();
	assert_eq!(count, 0, "starting an operation must not wake the Run");
	sqlx::query("UPDATE core_operations SET state = 'completed' WHERE id = $1")
		.bind(operation)
		.execute(db)
		.await
		.unwrap();
	let count: i64 = sqlx::query_scalar(
		"SELECT count(*) FROM run_activations WHERE run_id = $1 AND reason = 'operation_finished'",
	)
	.bind(run.id)
	.fetch_one(db)
	.await
	.unwrap();
	assert_eq!(count, 1, "finishing an operation must request activation");
	sqlx::query("UPDATE core_operations SET state = 'running' WHERE id = $1")
		.bind(operation)
		.execute(db)
		.await
		.unwrap();
	// A Task finishing after the decision to wait must survive the wait commit.
	let child = f
		.store
		.create_task(
			&scope,
			workspace.id,
			&serde_json::from_value(
				json!({"title":"Child", "description":"Wait race", "parent_id":task.id}),
			)
			.unwrap(),
			"alice",
			None,
		)
		.await
		.unwrap();
	sqlx::query("INSERT INTO authorization_task_origins (task_id, tenant, source_run_id, root_subject, subject_chain) VALUES ($1, 'acme', $2, 'alice', ARRAY['alice','fixture-agent'])")
		.bind(child.id).bind(run.id).execute(db).await.unwrap();
	sqlx::query("UPDATE tasks SET status = 'RUNNING', owner = 'alice' WHERE id = $1")
		.bind(child.id)
		.execute(db)
		.await
		.unwrap();
	let count: i64 = sqlx::query_scalar(
		"SELECT count(*) FROM run_activations WHERE run_id = $1 AND reason LIKE 'workspace_task:%'",
	)
	.bind(run.id)
	.fetch_one(db)
	.await
	.unwrap();
	assert_eq!(count, 0, "a Task starting must not wake its parent");
	let (old_phase, old_pending): (String, sqlx::types::Json<serde_json::Value>) =
		sqlx::query_as("SELECT phase, pending FROM runs WHERE id = $1")
			.bind(run.id)
			.fetch_one(db)
			.await
			.unwrap();
	// A finishing Task must not acquire the parent Run row, even while the
	// parent is committing its wait.
	let mut waiting_tx = db.begin().await.unwrap();
	sqlx::query("SELECT id FROM runs WHERE id = $1 FOR UPDATE")
		.bind(run.id)
		.fetch_one(&mut *waiting_tx)
		.await
		.unwrap();
	let db_for_task = db.clone();
	let child_id = child.id;
	let finishing = tokio::spawn(async move {
		sqlx::query("UPDATE tasks SET status = 'COMPLETED' WHERE id = $1")
			.bind(child_id)
			.execute(&db_for_task)
			.await
			.unwrap();
	});
	tokio::time::timeout(std::time::Duration::from_secs(3), finishing)
		.await
		.expect("Task transition blocked on its parent's Run row")
		.unwrap();
	let wait = aidash_domain::RunState::Waiting(Box::new(
		aidash_domain::run_state::WaitingState::Workspace {
			wake_at: Utc::now() + chrono::Duration::hours(1),
			cursor: 0,
			resume: aidash_domain::ResumeState::Thinking(Default::default()),
		},
	));
	let pending = aidash_domain::run_state::encode(&wait, &Default::default()).unwrap();
	sqlx::query("UPDATE runs SET phase = 'WAITING', pending = $2 WHERE id = $1")
		.bind(run.id)
		.bind(sqlx::types::Json(pending))
		.execute(&mut *waiting_tx)
		.await
		.unwrap();
	waiting_tx.commit().await.unwrap();

	// Two producers for the same Run allocate generations in commit order,
	// without locking the Run row in either transaction.
	let mut children = Vec::new();
	for title in ["First producer", "Second producer"] {
		let next = f
			.store
			.create_task(
				&scope,
				workspace.id,
				&serde_json::from_value(
					json!({"title":title,"description":"Event order","parent_id":task.id}),
				)
				.unwrap(),
				"alice",
				None,
			)
			.await
			.unwrap();
		sqlx::query("INSERT INTO authorization_task_origins (task_id, tenant, source_run_id, root_subject, subject_chain) VALUES ($1, 'acme', $2, 'alice', ARRAY['alice','fixture-agent'])")
			.bind(next.id).bind(run.id).execute(db).await.unwrap();
		children.push(next.id);
	}
	let mut first_tx = db.begin().await.unwrap();
	sqlx::query("UPDATE tasks SET status = 'COMPLETED' WHERE id = $1")
		.bind(children[0])
		.execute(&mut *first_tx)
		.await
		.unwrap();
	let mut claim_tx = db.begin().await.unwrap();
	sqlx::query("SELECT id FROM runs WHERE id = $1 FOR UPDATE NOWAIT")
		.bind(run.id)
		.fetch_one(&mut *claim_tx)
		.await
		.expect("uncommitted Task transition locked its parent's Run row");
	claim_tx.rollback().await.unwrap();
	let db_for_second = db.clone();
	let second_id = children[1];
	let second = tokio::spawn(async move {
		sqlx::query("UPDATE tasks SET status = 'COMPLETED' WHERE id = $1")
			.bind(second_id)
			.execute(&db_for_second)
			.await
			.unwrap();
	});
	tokio::time::sleep(std::time::Duration::from_millis(150)).await;
	assert!(
		!second.is_finished(),
		"producer generations must follow commit order"
	);
	first_tx.commit().await.unwrap();
	tokio::time::timeout(std::time::Duration::from_secs(3), second)
		.await
		.unwrap()
		.unwrap();
	let generations: Vec<i64> = sqlx::query_scalar("SELECT generation FROM run_activations WHERE run_id = $1 AND reason IN ($2, $3) ORDER BY generation")
		.bind(run.id).bind(format!("workspace_task:{}:COMPLETED", children[0]))
		.bind(format!("workspace_task:{}:COMPLETED", children[1]))
		.fetch_all(db).await.unwrap();
	assert_eq!(generations.len(), 2);
	let first_generation: i64 = sqlx::query_scalar(
		"SELECT generation FROM run_activations WHERE run_id = $1 AND reason = $2",
	)
	.bind(run.id)
	.bind(format!("workspace_task:{}:COMPLETED", children[0]))
	.fetch_one(db)
	.await
	.unwrap();
	assert_eq!(generations[0], first_generation);

	let activation = crate::bootstrap::activation_repository(crate::harness::Harness {
		federation: f.clone(),
	});
	use aidash_application::ports::activation::ActivationRepository as _;
	let mut scheduling = activation.claim_scope().await.unwrap();
	let raw = scheduling.lock_run(run.id).await.unwrap().unwrap();
	let now = Utc::now();
	let due =
		aidash_application::activation::due(scheduling.as_mut(), &raw, now, activation.node_id())
			.await
			.unwrap();
	assert!(
		due.is_some_and(|due| due <= now + chrono::Duration::seconds(3)),
		"the committed wait must see the Task event"
	);
	drop(scheduling);
	sqlx::query("UPDATE runs SET phase = $2, pending = $3 WHERE id = $1")
		.bind(run.id)
		.bind(old_phase)
		.bind(old_pending)
		.execute(db)
		.await
		.unwrap();

	// A wake reports only the newest 32 Task changes. The Run's cursor moves
	// past the older ones, so the wake must count what it left out.
	let cursor: i64 = sqlx::query_scalar(
		"SELECT COALESCE(MAX(generation), 0) FROM run_activations WHERE run_id = $1",
	)
	.bind(run.id)
	.fetch_one(db)
	.await
	.unwrap();
	for _ in 0..40 {
		sqlx::query("SELECT aidash_request_activation($1, $2)")
			.bind(run.id)
			.bind(format!("workspace_task:{}:COMPLETED", Uuid::new_v4()))
			.execute(db)
			.await
			.unwrap();
	}
	let generations: Vec<i64> = sqlx::query_scalar("SELECT generation FROM run_activations WHERE run_id = $1 AND generation > $2 AND reason LIKE 'workspace_task:%' ORDER BY generation")
		.bind(run.id).bind(cursor).fetch_all(db).await.unwrap();
	assert_eq!(generations.len(), 40);
	let executor = f.store.for_run(scope.clone());
	for (after, kept, dropped) in [
		(cursor, 32, 8),
		(generations[6], 32, 1),
		(generations[7], 32, 0),
		(generations[8], 31, 0),
	] {
		let news = aidash_application::ports::execution::ExecutionStore::workspace_events(
			&executor, &run, after,
		)
		.await
		.unwrap();
		assert_eq!(news.events.len(), kept, "after {after}");
		assert_eq!(news.dropped, dropped, "after {after}");
		// Oldest first, ending at the newest change: the cursor lands on it.
		assert_eq!(news.events.last().unwrap().generation, generations[39]);
		assert!(
			news.events
				.windows(2)
				.all(|pair| pair[0].generation < pair[1].generation)
		);
	}

	let token = request["egress"]["token"].as_str().unwrap();
	let digest = "a".repeat(64);
	// Admission must not wait behind an activation claim on the Run row.
	let mut claim = f.store.pool.fixture_driver().begin().await.unwrap();
	sqlx::query("SELECT id FROM runs WHERE id = $1 FOR UPDATE")
		.bind(run.id)
		.fetch_one(&mut *claim)
		.await
		.unwrap();
	let activation = crate::bootstrap::activation_repository(crate::harness::Harness {
		federation: f.clone(),
	});
	let mut claim_scope = activation.claim_scope().await.unwrap();
	let contention = claim_scope.lock_run(run.id).await.err().unwrap();
	assert!(matches!(
		contention,
		aidash_application::Error::RunLockContended
	));
	drop(claim_scope);
	// Reconciliation takes NO KEY UPDATE: it serializes state changes while
	// allowing the request's foreign-key check to acquire KEY SHARE. Neither
	// admission nor metering should queue behind that lock.
	let mut reconciliation = db.begin().await.unwrap();
	sqlx::query("SELECT id FROM core_operations WHERE id = $1 FOR NO KEY UPDATE")
		.bind(operation)
		.fetch_one(&mut *reconciliation)
		.await
		.unwrap();
	let mut competing_reconciliation = db.begin().await.unwrap();
	let locked =
		sqlx::query("SELECT id FROM core_operations WHERE id = $1 FOR NO KEY UPDATE NOWAIT")
			.bind(operation)
			.fetch_one(&mut *competing_reconciliation)
			.await
			.unwrap_err();
	assert_eq!(
		locked
			.as_database_error()
			.and_then(|error| error.code())
			.as_deref(),
		Some("55P03")
	);
	competing_reconciliation.rollback().await.unwrap();
	let gateway = Gateway::new(f.store.clone());
	let first = tokio::time::timeout(
		std::time::Duration::from_secs(5),
		gateway.admit(token, "images", "POST", "upload", &digest),
	)
	.await
	.expect("admission waited for the Run or Operation lock")
	.unwrap();
	tokio::time::timeout(
		std::time::Duration::from_secs(5),
		first.meter.charge(Direction::Sent, 0),
	)
	.await
	.expect("debit waited for the Run or Operation lock")
	.unwrap();
	reconciliation.rollback().await.unwrap();
	claim.rollback().await.unwrap();
	// A request-record insertion must not take a KEY SHARE lock on the Run:
	// activation still uses FOR UPDATE NOWAIT and must claim concurrently.
	let mut pending_request = db.begin().await.unwrap();
	sqlx::query("INSERT INTO egress_requests (id, tenant, run_id, operation_id, session_id, service, method, path, query_digest, status, sent_bytes, received_bytes, duration_ms, via, error, secret_id, secret_version, secret_host, created_at) SELECT $1, tenant, run_id, operation_id, session_id, service, method, path, query_digest, status, sent_bytes, received_bytes, duration_ms, via, error, secret_id, secret_version, secret_host, created_at FROM egress_requests WHERE id = $2")
		.bind(Uuid::new_v4())
		.bind(first.request)
		.execute(&mut *pending_request)
		.await
		.unwrap();
	let mut claim_while_admitting = activation.claim_scope().await.unwrap();
	assert!(
		claim_while_admitting
			.lock_run(run.id)
			.await
			.unwrap()
			.is_some(),
		"uncommitted request insertion blocked Run activation"
	);
	drop(claim_while_admitting);
	pending_request.rollback().await.unwrap();
	assert!(matches!(
		Gateway::new(f.store.clone())
			.admit(
				&format!("{operation}.{}", "a".repeat(43)),
				"images",
				"GET",
				"forged",
				&digest
			)
			.await,
		Err(aidash_application::Error::Forbidden)
	));
	// A command's request waits behind an Authority Change, and the change does
	// not wait for it, since the step that started the command has gone on (ADR 0121).
	// A running step stands in for the work the change waits for. The request
	// ends refused after its checks, so that it does not count against the budget.
	let applied_before: chrono::DateTime<Utc> =
		sqlx::query_scalar("SELECT updated_at FROM authorization_bundles WHERE tenant = 'acme'")
			.fetch_one(db)
			.await
			.unwrap();
	let mut running = f.store.pool.begin(&TEST_FIXTURE).await.unwrap();
	AuthorizationBundle::lock_frozen(&mut *running, "acme", false)
		.await
		.unwrap();
	let change = {
		let store = f.store.clone();
		tokio::spawn(async move {
			let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
			AuthorizationBundle::lock_frozen(&mut *tx, "acme", true).await?;
			let (sql, values) = Query::update()
				.table(Alias::new("authorization_bundles"))
				.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
				.and_where(Expr::col("tenant").eq(Expr::value("acme")))
				.build(PostgresQueryBuilder);
			let executor: &mut dyn TransactionExecutor = &mut *tx;
			executor.execute(&sql, convert_values(values)).await?;
			tx.commit().await?;
			Ok::<_, crate::Error>(())
		})
	};
	tokio::time::timeout(std::time::Duration::from_secs(5), async {
		while crate::database::authority_change::is_clear(&f.store.pool, "acme").await {
			tokio::time::sleep(std::time::Duration::from_millis(5)).await;
		}
	})
	.await
	.expect("the change raised its flag");
	let admitting = {
		let (gateway, token, digest) = (
			Gateway::new(f.store.clone()),
			token.to_owned(),
			digest.clone(),
		);
		tokio::spawn(async move {
			gateway
				.admit(&token, "absent", "GET", "download", &digest)
				.await
		})
	};
	tokio::time::sleep(std::time::Duration::from_millis(500)).await;
	assert!(
		!admitting.is_finished(),
		"the request passed a waiting change"
	);
	assert!(
		!change.is_finished(),
		"the change did not wait for the step"
	);
	running.commit().await.unwrap();
	// Had the request waited after taking its share of the policy, the change
	// would be waiting for it now, and would only end by being refused.
	tokio::time::timeout(std::time::Duration::from_secs(5), change)
		.await
		.expect("the change waited for the request")
		.unwrap()
		.expect("the change is applied");
	let applied_after: chrono::DateTime<Utc> =
		sqlx::query_scalar("SELECT updated_at FROM authorization_bundles WHERE tenant = 'acme'")
			.fetch_one(db)
			.await
			.unwrap();
	assert!(
		applied_after > applied_before,
		"the change wrote the policy"
	);
	assert!(matches!(
		tokio::time::timeout(std::time::Duration::from_secs(5), admitting)
			.await
			.expect("the request went on once the change ended")
			.unwrap(),
		Err(aidash_application::Error::Forbidden)
	));
	first.meter.charge(Direction::Sent, 6).await.unwrap();
	// A fresh gateway instance sees the same durable operation budget.
	let gateway = Gateway::new(f.store.clone());
	let admissions = futures_util::future::join_all(
		(0..8).map(|_| gateway.admit(token, "images", "GET", "download", &digest)),
	)
	.await;
	assert_eq!(admissions.iter().filter(|result| result.is_ok()).count(), 1);
	let second = admissions.into_iter().find_map(Result::ok).unwrap();
	let attempts = (0..8).map(|i| {
		let meter = if i % 2 == 0 {
			first.meter.clone()
		} else {
			second.meter.clone()
		};
		async move { meter.charge(Direction::Sent, 1).await }
	});
	let results = futures_util::future::join_all(attempts).await;
	// Debits are serialized on the budgets and always commit the bytes the
	// gateway already streamed. The debit that reaches the limit, and every
	// later one, reports exhaustion after committing.
	assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 3);
	assert!(results.iter().filter_map(|r| r.as_ref().err()).all(
		|e| matches!(e, aidash_application::Error::Conflict(code) if code == aidash_domain::egress::LIMIT_EXCEEDED)
	));
	second.meter.charge(Direction::Received, 11).await.unwrap();
	assert!(matches!(
		first.meter.charge(Direction::Received, 2).await,
		Err(aidash_application::Error::Conflict(code)) if code == aidash_domain::egress::LIMIT_EXCEEDED
	));
	// A record holds no times until its outcome is settled.
	let mut tx = f.store.pool.begin(&TEST_FIXTURE).await.unwrap();
	let unsettled = RequestRecord::objects()
		.filter(RequestRecord::field_id().eq(first.request))
		.all_with_executor(&mut *tx)
		.await
		.unwrap()
		.pop()
		.unwrap();
	assert_eq!((unsettled.admission_ms, unsettled.settle_ms), (None, None));
	assert_eq!((unsettled.status, unsettled.error.as_deref()), (0, None));
	tx.commit().await.unwrap();
	first.meter.finish(201, times(20), None).await.unwrap();
	second.meter.finish(200, times(25), None).await.unwrap();
	assert!(matches!(
		Gateway::new(f.store.clone())
			.admit(token, "images", "GET", "third", &digest)
			.await,
		Err(aidash_application::Error::Conflict(_))
	));
	let mut tx = f.store.pool.begin(&TEST_FIXTURE).await.unwrap();
	let total = Budget::objects()
		.filter(Budget::field_operation_id().eq(operation))
		.filter(Budget::field_service().eq(""))
		.all_with_executor(&mut *tx)
		.await
		.unwrap()
		.pop()
		.unwrap();
	// Overshoot is bounded by one debit per stream.
	assert_eq!(
		(total.sent_bytes, total.received_bytes, total.requests),
		(14, 13, 2)
	);
	let records = RequestRecord::objects()
		.filter(RequestRecord::field_operation_id().eq(operation))
		.all_with_executor(&mut *tx)
		.await
		.unwrap();
	assert_eq!(records.len(), 2);
	assert_eq!(records.iter().map(|r| r.sent_bytes).sum::<i64>(), 14);
	assert_eq!(records.iter().map(|r| r.received_bytes).sum::<i64>(), 13);
	assert!(
		records
			.iter()
			.all(|r| r.error.is_none() && r.query_digest == digest && r.status >= 200)
	);
	// Run details read these records through the domain type.
	for record in &records {
		let shown: aidash_domain::egress::RequestRecord =
			serde_json::from_value(serde_json::to_value(record).unwrap()).unwrap();
		assert_eq!((shown.admission_ms, shown.settle_ms), (Some(3), Some(4)));
	}
	tx.commit().await.unwrap();
	let preview_meter = live_preview_credentials(&f, &identity, &run, area.id).await;
	auth.set_catalog(
		"acme",
		&run.node,
		&aidash_domain::registry::EntityRef {
			id: "images".into(),
			version: "1.0.0".into(),
		},
		1,
		false,
		"deployment_administrator",
	)
	.await
	.unwrap();
	assert!(matches!(
		Gateway::new(f.store.clone())
			.admit(token, "images", "GET", "revoked", &digest)
			.await,
		Err(aidash_application::Error::Forbidden)
	));
	// Metering rechecks Run control, including on an already admitted stream.
	auth.set_catalog(
		"acme",
		&run.node,
		&aidash_domain::registry::EntityRef {
			id: "images".into(),
			version: "1.0.0".into(),
		},
		2,
		true,
		"deployment_administrator",
	)
	.await
	.unwrap();
	first.meter.charge(Direction::Sent, 0).await.unwrap();
	// Failing to find out whether the operation may go on is not a decision about
	// it: nothing is committed, so charging the same bytes again counts them once.
	let before = sent_so_far(&f, operation, first.request).await;
	assert_eq!(before, (14, 10));
	sqlx::query(
		"UPDATE core_operations SET subjects = jsonb_build_object('unreadable', subjects) WHERE id = $1",
	)
	.bind(operation)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	let failure = first.meter.charge(Direction::Sent, 5).await.unwrap_err();
	assert!(
		!matches!(
			failure,
			aidash_application::Error::Conflict(_)
				| aidash_application::Error::Forbidden
				| aidash_application::Error::Unauthorized
		),
		"{failure}"
	);
	assert_eq!(
		sent_so_far(&f, operation, first.request).await,
		before,
		"a failure that decides nothing commits nothing"
	);
	sqlx::query("UPDATE core_operations SET subjects = subjects -> 'unreadable' WHERE id = $1")
		.bind(operation)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	// The gateway's retry. The budget is spent, so this is a decision, and one
	// that commits the bytes it reports on.
	assert!(matches!(
		first.meter.charge(Direction::Sent, 5).await,
		Err(aidash_application::Error::Conflict(code)) if code == aidash_domain::egress::LIMIT_EXCEEDED
	));
	assert_eq!(
		sent_so_far(&f, operation, first.request).await,
		(before.0 + 5, before.1 + 5),
		"the bytes are counted once, in the budget and in the record"
	);
	// Egress asks only whether the Run may still run, whatever it has read
	// (ADR 0116). The 30 second Read Recheck covers the reads.
	let (active, statements) = statements::recorded(first.meter.check_active()).await;
	active.unwrap();
	assert!(!statements.is_empty(), "the counter sees statements");
	assert!(
		statements
			.iter()
			.all(|statement| !statement.contains("authorization_run_reads")),
		"{statements:#?}"
	);
	// Withdrawing the Agent's own Catalog Approval leaves the policy revision as
	// it was, and the next check refuses the request all the same.
	let reference = aidash_domain::registry::EntityRef {
		id: agent.id.clone(),
		version: agent.version.clone(),
	};
	let approved = auth
		.catalog("acme", &run.node)
		.await
		.unwrap()
		.into_iter()
		.find(|binding| binding.entry_id == agent.id && binding.entry_version == agent.version)
		.unwrap()
		.revision;
	let revision = auth.snapshot("acme").await.unwrap().revision;
	auth.set_catalog(
		"acme",
		&run.node,
		&reference,
		approved,
		false,
		"deployment_administrator",
	)
	.await
	.unwrap();
	assert_eq!(auth.snapshot("acme").await.unwrap().revision, revision);
	assert!(matches!(
		first.meter.check_active().await,
		Err(aidash_application::Error::Forbidden)
	));
	auth.set_catalog(
		"acme",
		&run.node,
		&reference,
		approved + 1,
		true,
		"deployment_administrator",
	)
	.await
	.unwrap();
	first.meter.check_active().await.unwrap();
	f.store
		.control(&scope, run.id, aidash_domain::RunControlAction::Cancel)
		.await
		.unwrap();
	// A stopped Run's operation is inactive; the preview's session has ended.
	assert!(matches!(
		first.meter.check_active().await,
		Err(aidash_application::Error::Conflict(code)) if code == aidash_domain::egress::OPERATION_INACTIVE
	));
	assert!(matches!(
		preview_meter.check_active().await,
		Err(aidash_application::Error::Conflict(code)) if code == aidash_domain::egress::OPERATION_EXPIRED
	));
	// An ended operation is reported as such, not as a revoked authority or an
	// exhausted budget, and the bytes streamed before the refusal still count.
	assert!(matches!(
		first.meter.charge(Direction::Sent, 3).await,
		Err(aidash_application::Error::Conflict(code)) if code == aidash_domain::egress::OPERATION_INACTIVE
	));
	let mut tx = f.store.pool.begin(&TEST_FIXTURE).await.unwrap();
	let total = Budget::objects()
		.filter(Budget::field_operation_id().eq(operation))
		.filter(Budget::field_service().eq(""))
		.all_with_executor(&mut *tx)
		.await
		.unwrap()
		.pop()
		.unwrap();
	assert_eq!(total.sent_bytes, 22);
	let record = RequestRecord::objects()
		.filter(RequestRecord::field_id().eq(first.request))
		.all_with_executor(&mut *tx)
		.await
		.unwrap()
		.pop()
		.unwrap();
	assert_eq!(record.sent_bytes, 6 + 4 + 5 + 3);
	tx.commit().await.unwrap();
	// A command the Read Recheck withdrew lost its authority rather than ending,
	// and its requests say so, even after the Run has stopped too.
	sqlx::query(
		"UPDATE core_operations SET result = result || '{\"error\":\"AUTHORITY_WITHDRAWN\"}' WHERE id=$1",
	)
	.bind(operation)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert!(matches!(
		first.meter.check_active().await,
		Err(aidash_application::Error::Forbidden)
	));
	assert!(matches!(
		first.meter.charge(Direction::Sent, 1).await,
		Err(aidash_application::Error::Forbidden)
	));
}

/// The operation's total sent bytes, and those of one of its requests.
async fn sent_so_far(
	f: &crate::federation::Federation,
	operation: Uuid,
	request: Uuid,
) -> (i64, i64) {
	let mut tx = f.store.pool.begin(&TEST_FIXTURE).await.unwrap();
	let total = Budget::objects()
		.filter(Budget::field_operation_id().eq(operation))
		.filter(Budget::field_service().eq(""))
		.all_with_executor(&mut *tx)
		.await
		.unwrap()
		.pop()
		.unwrap()
		.sent_bytes;
	let record = RequestRecord::objects()
		.filter(RequestRecord::field_id().eq(request))
		.all_with_executor(&mut *tx)
		.await
		.unwrap()
		.pop()
		.unwrap()
		.sent_bytes;
	tx.commit().await.unwrap();
	(total, record)
}

fn times(duration_ms: u64) -> aidash_application::egress::RequestTimes {
	aidash_application::egress::RequestTimes {
		duration_ms,
		admission_ms: 3,
		settle_ms: 4,
	}
}

async fn live_preview_credentials(
	f: &crate::federation::Federation,
	identity: &crate::authorization::identity::SubjectIdentity,
	run: &aidash_domain::Run,
	area: Uuid,
) -> std::sync::Arc<dyn aidash_application::egress::Meter> {
	use crate::apps::execution::repositories::egress::{page_token, session_token};
	use secrecy::ExposeSecret;
	let preview = Uuid::now_v7();
	let now = Utc::now();
	let expires = now + chrono::Duration::hours(2);
	sqlx::query("INSERT INTO core_live_previews (id,tenant,run_id,area_id,revision,port,state,idle_seconds,absolute_seconds,started_at,last_request_at,expires_at) VALUES ($1,'acme',$2,$3,1,3000,'running',900,7200,$4,$4,$5)")
		.bind(preview).bind(run.id).bind(area).bind(now).bind(expires)
		.execute(f.store.pool.fixture_driver()).await.unwrap();
	sqlx::query("INSERT INTO egress_sessions (id,tenant,run_id,live_preview_id,principal,credential_id,expires_at,max_sent_bytes,max_received_bytes,max_requests) VALUES ($1,'acme',$2,$1,'alice',$3,$4,100,100,10)")
		.bind(preview).bind(run.id).bind(identity.credential_id).bind(expires)
		.execute(f.store.pool.fixture_driver()).await.unwrap();
	let pod = session_token(preview, expires, &f.store).unwrap();
	let page = page_token(preview, expires, &f.store).unwrap();
	let gateway = Gateway::new(f.store.clone());
	let digest = "b".repeat(64);
	// A Pod cannot change its credential's kind to impersonate the page path,
	// and a page cannot turn its credential into one that attaches the Secret.
	for forged in [
		pod.replacen("session:", "page:", 1),
		page.replacen("page:", "session:", 1),
	] {
		assert!(matches!(
			gateway
				.admit(&forged, "secured-images", "GET", "/", &digest)
				.await,
			Err(aidash_application::Error::Forbidden)
		));
	}
	let pod = gateway
		.admit(&pod, "secured-images", "GET", "/", &digest)
		.await
		.unwrap();
	let preview_meter = pod.meter.clone();
	assert_eq!(
		pod.credential.as_ref().unwrap().expose_secret(),
		"live-preview-secret-canary"
	);
	let page = gateway
		.admit(&page, "secured-images", "GET", "/", &digest)
		.await
		.unwrap();
	assert!(
		page.credential.is_none(),
		"the page reaches the same service without its Tenant Secret"
	);
	for (admission, via, host) in [
		(&pod, "pod", Some("api.example.test")),
		(&page, "page", None),
	] {
		let mut tx = f
			.store
			.pool
			.begin(&TenancyScope::tenant(&identity.tenant).unwrap())
			.await
			.unwrap();
		let unsettled = RequestRecord::objects()
			.filter(RequestRecord::field_id().eq(admission.request))
			.all_with_executor(&mut *tx)
			.await
			.unwrap()
			.pop()
			.unwrap();
		assert_eq!((unsettled.status, unsettled.error.as_deref()), (0, None));
		tx.commit().await.unwrap();
		admission.meter.finish(200, times(1), None).await.unwrap();
		let (recorded, secret_host): (String, Option<String>) =
			sqlx::query_as("SELECT via, secret_host FROM egress_requests WHERE id = $1")
				.bind(admission.request)
				.fetch_one(f.store.pool.fixture_driver())
				.await
				.unwrap();
		assert_eq!(recorded, via);
		// Only the request that carried the Tenant Secret names where it went.
		assert_eq!(secret_host.as_deref(), host);
	}
	// A brokered preview mints once per session and records only the pinned identity.
	let existing = f.store.tenant_secrets.as_ref().unwrap();
	let calls = std::sync::Arc::new(RecordedCalls::default());
	let mut store = f.store.clone();
	store.tenant_secrets = Some(std::sync::Arc::new(
		aidash_application::tenant_secrets::Service {
			repository: existing.repository.clone(),
			store: existing.store.clone(),
			reader: None,
			calls: Some(calls.clone()),
			fingerprint_key: existing.fingerprint_key.clone(),
			max_per_tenant: existing.max_per_tenant,
		},
	));
	let gateway = Gateway::new(store);
	let token = session_token(preview, expires, &f.store).unwrap();
	for _ in 0..2 {
		let admitted = gateway
			.admit(&token, "secured-images", "GET", "/", &digest)
			.await
			.unwrap();
		assert!(admitted.credential.is_none());
		assert_eq!(
			admitted.route.as_ref().unwrap().token.expose_secret(),
			"fixture-route-token"
		);
		admitted.meter.finish(200, times(1), None).await.unwrap();
		let row: (Option<Uuid>, Option<String>, Option<String>) = sqlx::query_as(
			"SELECT secret_id, secret_version, secret_host FROM egress_requests WHERE id = $1",
		)
		.bind(admitted.request)
		.fetch_one(f.store.pool.fixture_driver())
		.await
		.unwrap();
		assert!(row.0.is_some());
		assert_eq!(row.1.as_deref(), Some("1"));
		assert_eq!(row.2.as_deref(), Some("api.example.test"));
	}
	let grants = calls.0.lock().unwrap();
	assert_eq!(grants.len(), 1);
	assert_eq!(grants[0].tenant, "acme");
	assert_eq!(grants[0].destination.origin, "https://api.example.test");
	assert_eq!(grants[0].destination.path_prefix, "/");
	assert_eq!(grants[0].methods, vec!["GET"]);
	assert!(
		matches!(&grants[0].subject, aidash_capability::tenant_secret::SecretSubject::Operation { operation, tenant } if *operation == preview && tenant == "acme")
	);
	preview_meter
}

struct RunnerReply {
	started: f64,
}
#[async_trait::async_trait]
impl reinhardt::Handler for RunnerReply {
	async fn handle(&self, request: reinhardt::Request) -> reinhardt::http::ViewResult<Response> {
		assert_eq!(
			request.headers["authorization"].to_str().unwrap(),
			format!("Bearer {}", std::env::var(RUNNER_TOKEN_ENV).unwrap())
		);
		Ok(Response::ok()
			.with_json(&json!({"status":"running","started_at":self.started}))
			.unwrap())
	}
}

#[derive(Default)]
struct RecordedCalls(std::sync::Mutex<Vec<aidash_application::tenant_secrets::SecretCallGrant>>);
#[async_trait::async_trait]
impl aidash_application::tenant_secrets::SecretCalls for RecordedCalls {
	async fn route(
		&self,
		grant: aidash_application::tenant_secrets::SecretCallGrant,
	) -> aidash_application::Result<aidash_application::tenant_secrets::SecretRoute> {
		self.0.lock().unwrap().push(grant);
		Ok(aidash_application::tenant_secrets::SecretRoute {
			base: "https://broker.example/v1/forward".parse().unwrap(),
			token: "fixture-route-token".into(),
		})
	}
}
