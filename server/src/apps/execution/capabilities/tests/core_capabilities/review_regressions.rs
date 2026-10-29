use super::*;
use aidash_application::tenancy::TEST_FIXTURE;
use base64::{Engine, engine::general_purpose::STANDARD};
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};

#[rstest::fixture]
async fn withdrawn_operation_fixture(
	#[future] capability_fixture: CoreFixture,
) -> (CoreFixture, aidash_server::domain::Run, Value, Uuid) {
	let c = Box::pin(capability_fixture).await;
	let run = admit(&c).await;
	let (status, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	let id = Uuid::new_v4();
	// Recover a committed dispatch intent whose originating credential was lost.
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("core_operations"))
			.columns(
				[
					"id",
					"area_id",
					"run_id",
					"tenant",
					"principal",
					"credential_id",
					"subjects",
					"request_key",
					"digest",
					"kind",
					"state",
					"epoch",
					"generation",
					"revision",
					"policy_revision",
					"input",
					"result",
				]
				.map(Alias::new),
			)
			.from_subquery(((1..=17).map(|i| Expr::cust(format!("${i}")))).fold(
				reinhardt::query::Query::select(),
				|mut select, expr| {
					select.expr(expr);
					select
				},
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(serde_json::from_value::<Uuid>(area["id"].clone()).unwrap())
	.bind(run.id)
	.bind("acme")
	.bind("alice")
	.bind(Uuid::new_v4())
	.bind(json!(["alice"]))
	.bind(id.to_string())
	.bind("immutable-intent")
	.bind("shell")
	.bind("prepared")
	.bind(area["epoch"].as_i64().unwrap())
	.bind(area["generation"].as_i64().unwrap())
	.bind(area["revision"].as_i64().unwrap())
	.bind(2_i64)
	.bind(json!({"command":"must never execute","seconds":1}))
	.bind(json!({}))
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	{
		let query_bind_1 = serde_json::from_value::<Uuid>(area["id"].clone()).unwrap();
		sqlx::query(
			&Query::update()
				.table(Alias::new("core_areas"))
				.value(Alias::new("state"), "running")
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())))
				.to_string(PostgresQueryBuilder),
		)
		.execute(c.f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	(c, run, area, id)
}

#[rstest::rstest]
#[tokio::test]
async fn authority_withdrawal_before_dispatch_keeps_saved_files_usable(
	#[future]
	#[from(running_withdrawn_operation_fixture)]
	withdrawn_operation_fixture: (
		CoreFixture,
		aidash_server::domain::Run,
		Value,
		Uuid,
		CapabilityWorker,
	),
) {
	let (c, run, area, id, worker) = Box::pin(withdrawn_operation_fixture).await;
	let stop = worker.stop.clone();
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
	loop {
		let state: String = {
			let query_bind_1 = id;
			sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("state"))
					.from(Alias::new("core_operations"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(c.f.store.pool.fixture_driver())
			.await
		}
		.unwrap();
		if state == "withdrawn" {
			break;
		}
		assert!(tokio::time::Instant::now() < deadline, "{state}");
		tokio::time::sleep(std::time::Duration::from_millis(50)).await;
	}
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	let state: String = {
		let query_bind_1 = serde_json::from_value::<Uuid>(area["id"].clone()).unwrap();
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("state"))
				.from(Alias::new("core_areas"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert_eq!(
		state, "active",
		"an undispatched operation has no uncertain effects"
	);
	let (status,patched) = request(&c.app,&c.token,"POST",&path_for_patch(run.id),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"preconditions":{"saved.txt":null},"patch":"*** Begin Patch\n*** Add File: saved.txt\n+still usable\n*** End Patch"})).await;
	assert_eq!(status, 200, "{patched}");
	c.close().await;
}

#[rstest::fixture]
async fn extraction_queue_fixture(
	#[future] capability_fixture: CoreFixture,
) -> (CoreFixture, String) {
	let mut c = Box::pin(capability_fixture).await;
	let bytes = b"private original";

	let mut uploads = vec![];
	for index in 0..9 {
		let (status, credential) = request(
			&c.app,
			&c.f.config.api_token,
			"POST",
			"/api/authorization/acme/credentials",
			json!({"subject":"alice"}),
		)
		.await;
		assert_eq!(status, 200, "{credential}");
		let token = credential["token"].as_str().unwrap();
		let (status, upload) = request(&c.app,token,"POST","/api/references/uploads",json!({"idempotency_key":Uuid::new_v4(),"name":format!("record-{index}.txt"),"media_type":"text/plain","size":bytes.len(),"digest":aidash_server::capabilities::objects::digest(bytes)})).await;
		assert_eq!(status, 200, "{upload}");
		let id: Uuid = serde_json::from_value(upload["reference_id"].clone()).unwrap();
		let path = format!("/api/references/{id}");
		assert_eq!(
			request(
				&c.app,
				token,
				"POST",
				&format!("{path}/chunks"),
				json!({"offset":0,"data":STANDARD.encode(bytes)})
			)
			.await
			.0,
			200
		);
		assert_eq!(
			request(
				&c.app,
				token,
				"POST",
				&format!("{path}/commit"),
				Value::Null
			)
			.await
			.0,
			200
		);
		uploads.push((
			id,
			path,
			credential["credential"]["id"].as_str().unwrap().to_owned(),
		));
	}
	// Revoke the credentials of the actual first scheduling batch without
	// rewriting/reordering its reference rows. The ninth upload stays valid.
	let first: Vec<Uuid> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("core_records"))
			.and_where(Expr::col(Alias::new("kind")).eq("reference"))
			.and_where(Expr::col(Alias::new("state")).eq("extracting"))
			.limit(8)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(first.len(), 8);
	let last = uploads
		.iter()
		.find(|(id, _, _)| !first.contains(id))
		.unwrap()
		.1
		.clone();
	for (id, _, credential) in uploads {
		if first.contains(&id) {
			let (status, revoked) = request(
				&c.app,
				&c.f.config.api_token,
				"POST",
				&format!("/api/authorization/acme/credentials/{credential}/revoke"),
				json!({}),
			)
			.await;
			assert_eq!(status, 200, "{revoked}");
		}
	}
	// Valid undispatched work can finish rollback without a runtime. Stale
	// credentials must not monopolize the first eight extraction queue slots.
	let mut profile = (*c.f.store.capabilities.0).clone();
	profile.admission = false;
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	c.app.context.set_singleton(c.f.clone());
	(c, last)
}

#[rstest::rstest]
#[tokio::test]
async fn stale_extraction_credentials_do_not_starve_later_uploads(
	#[future]
	#[from(running_extraction_queue_fixture)]
	extraction_queue_fixture: (CoreFixture, String, CapabilityWorker),
) {
	let (c, path, worker) = Box::pin(extraction_queue_fixture).await;
	let stop = worker.stop.clone();
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
	let finished = loop {
		let (status, state) = request(&c.app, &c.token, "GET", &path, Value::Null).await;
		assert_eq!(status, 200, "{state}");
		if state["state"] == "ready" || tokio::time::Instant::now() >= deadline {
			break state;
		}
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	};
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	assert_eq!(
		finished["state"], "ready",
		"stale credentials must not starve later work: {finished}"
	);
	assert_eq!(finished["extraction_state"], "admission_disabled");
	let (status, original) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("{path}/download"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{original}");
	assert_eq!(
		STANDARD.decode(original["data"].as_str().unwrap()).unwrap(),
		b"private original"
	);
	c.close().await;
}

#[rstest::fixture]
#[cfg(feature = "capability-runtime-tests")]
async fn full_mount_fixture(
	#[future] runtime_fixture: CoreFixture,
) -> (CoreFixture, aidash_server::domain::Run) {
	let mut c = Box::pin(runtime_fixture).await;
	let run = admit(&c).await;
	let skills: Value = {
		let query_bind_1 = run.id;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("data"))
				.from(Alias::new("core_records"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	let mut profile = (*c.f.store.capabilities.0).clone();
	profile.working_bytes = skills
		.as_array()
		.unwrap()
		.iter()
		.flat_map(|skill| skill["files"].as_array().unwrap())
		.map(|file| file["size"].as_u64().unwrap())
		.sum();
	assert!(profile.working_bytes > 1);
	// The admitted Binding remains exact when runtime availability changes.
	// These calls must enforce writable capacity before leaving a dispatch intent.
	profile.runner = None;
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	c.app.context.set_singleton(c.f.clone());
	(c, run)
}

#[rstest::rstest]
#[cfg(feature = "capability-runtime-tests")]
#[case(0, "WORKING_QUOTA_EXCEEDED")]
#[case(1, "RUNTIME_UNAVAILABLE")]
#[tokio::test]
async fn mounted_skills_require_writable_capacity_before_operation_commit(
	#[future] full_mount_fixture: (CoreFixture, aidash_server::domain::Run),
	#[case] writable_bytes: u64,
	#[case] expected_error: &str,
) {
	let (mut c, run) = Box::pin(full_mount_fixture).await;
	let mut profile = (*c.f.store.capabilities.0).clone();
	profile.working_bytes += writable_bytes;
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	c.app.context.set_singleton(c.f.clone());
	let (status, denied) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/shell", run.id),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"command":"printf must-not-run"}),
	)
	.await;
	assert_eq!(status, 409, "{denied}");
	assert!(
		denied["error"]["code"]
			.as_str()
			.unwrap()
			.starts_with(expected_error),
		"{denied}"
	);
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(area["state"], "active");
	let count: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(reinhardt::query::Func::count(
				Expr::col(Alias::new("id")).into(),
			))
			.from(Alias::new("core_operations"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(
		count, 0,
		"rejected admission must not leave a durable dispatch intent"
	);
	c.close().await;
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

use reinhardt::query::SimpleExpr;

#[rstest::fixture]
async fn running_withdrawn_operation_fixture(
	#[future] withdrawn_operation_fixture: (CoreFixture, aidash_server::domain::Run, Value, Uuid),
	worker_control: WorkerControl,
) -> (
	CoreFixture,
	aidash_server::domain::Run,
	Value,
	Uuid,
	CapabilityWorker,
) {
	let (c, run, area, id) = Box::pin(withdrawn_operation_fixture).await;
	let worker = CapabilityWorker {
		stop: worker_control.stop,
		handle: tokio::spawn(aidash_server::capabilities::operations::run(
			c.f.store.clone(),
			worker_control.receiver,
		)),
	};
	(c, run, area, id, worker)
}

#[rstest::fixture]
async fn running_extraction_queue_fixture(
	#[future] extraction_queue_fixture: (CoreFixture, String),
	worker_control: WorkerControl,
) -> (CoreFixture, String, CapabilityWorker) {
	let (c, path) = Box::pin(extraction_queue_fixture).await;
	let worker = CapabilityWorker {
		stop: worker_control.stop,
		handle: tokio::spawn(aidash_server::capabilities::operations::run(
			c.f.store.clone(),
			worker_control.receiver,
		)),
	};
	(c, path, worker)
}

#[rstest::rstest]
#[tokio::test]
async fn cached_skill_context_rechecks_descriptor_authority_before_model_retry(
	#[from(retry_calls)] calls: Arc<std::sync::atomic::AtomicUsize>,
	#[from(retry_router)]
	#[with(calls.clone())]
	_router: Arc<reinhardt::ServerRouter>,
	#[from(upstream_fixtures::provider_transport)]
	#[with(_router.clone())]
	provider: upstream_fixtures::UpstreamFuture,
	#[from(provider_endpoint)]
	#[with(provider.clone())]
	_endpoint: EndpointFuture,
	#[from(capability_fixture)]
	#[with("aidash://execution-test",_endpoint.clone())]
	core: CoreFuture,
) {
	use std::sync::atomic::Ordering;
	let c = core.await;
	let server = provider.await;
	let admitted = admit(&c).await;
	let harness = aidash_server::harness::Harness {
		federation: c.f.clone(),
	};
	harness.worker_once().await.unwrap();
	harness.worker_once().await.unwrap();
	assert_eq!(calls.load(Ordering::SeqCst), 1);
	let retry = c.f.store.run(&TEST_FIXTURE, admitted.id).await.unwrap();
	let observed = retry
		.context
		.source_observation
		.as_ref()
		.expect("Source read precedes model HTTP");
	assert!(
		!observed.content["skill_context"]
			.as_str()
			.unwrap()
			.is_empty()
	);
	let content = observed.content.clone();
	let mut policy = c.policy.clone();
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"withdraw-skill-list","effect":"deny","subjects":{"any":true},"actions":["tool.invoke"],"resources":{"kinds":["tool"],"ids":[aidash_domain::registry::bindings::QualifiedRef::builtin(c.f.config.deployment.as_str(),"skill_list").resource_id()]}}));
	crate::common::replace_policy(&c.f.store, "acme", policy.clone()).await;
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
	let blocked = loop {
		harness.worker_once().await.unwrap();
		let current = c.f.store.run(&TEST_FIXTURE, admitted.id).await.unwrap();
		if current.control == aidash_server::domain::RunControl::Paused {
			break current;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"cached context must not bypass revoked authority: {current:?}"
		);
		tokio::time::sleep(std::time::Duration::from_millis(50)).await;
	};
	assert_eq!(
		calls.load(Ordering::SeqCst),
		1,
		"revocation must prevent redisclosure to the model"
	);
	// The pause names the tool and the check that refused it (#530). The
	// withdrawal is a Tenant policy deny, so the check is `policy`, not
	// Catalog Approval.
	assert_eq!(
		blocked.error.as_deref(),
		Some("skill_list: tool.invoke was denied")
	);
	assert_eq!(
		blocked.context.source_observation.as_ref().unwrap().content,
		content,
		"the retry retains its original observation"
	);
	for path in [
		"/api/state".to_owned(),
		format!("/api/runs/{}", admitted.id),
	] {
		let (status, public) = request(&c.app, &c.token, "GET", &path, Value::Null).await;
		assert_eq!(status, 200, "{public}");
		let inspected = if path == "/api/state" {
			public["runs"]
				.as_array()
				.unwrap()
				.iter()
				.find(|run| run["id"] == json!(admitted.id))
				.unwrap()
		} else {
			&public["run"]
		};
		assert!(inspected["context"].is_object());
		assert!(
			inspected["context"].get("source_observation").is_none(),
			"cached Source text must not be a public inspection field"
		);
	}
	drop(server);
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn direct_host_http_rejects_a_withdrawn_bound_tool_before_creating_an_operation(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let identity = &run
		.context
		.binding_snapshot
		.as_ref()
		.unwrap()
		.operation("outbound_get")
		.unwrap()
		.identity;
	let mut policy = c.policy.clone();
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"withdraw-bound-tool-read",
		"effect":"deny",
		"subjects":{"any":true},
		"actions":["registry.read"],
		"resources":{"kinds":["tool"],"ids":[identity.resource_id()]}
	}));
	crate::common::replace_policy(&c.f.store, "acme", policy.clone()).await;
	let (status, body) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/outbound", run.id),
		json!({"idempotency_key":Uuid::new_v4(),"url":"https://example.com/data"}),
	)
	.await;
	assert_eq!(status, 403, "{body}");
	let count: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(reinhardt::query::Func::count(
				Expr::col(Alias::new("id")).into(),
			))
			.from(Alias::new("core_operations"))
			.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run.id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(
		count, 0,
		"withdrawn binding must not create a Host operation"
	);
	c.close().await;
}

#[rstest::fixture]
fn retry_calls() -> Arc<std::sync::atomic::AtomicUsize> {
	Arc::new(std::sync::atomic::AtomicUsize::new(0))
}
#[rstest::fixture]
fn retry_router(retry_calls: Arc<std::sync::atomic::AtomicUsize>) -> Arc<reinhardt::ServerRouter> {
	Arc::new(
		reinhardt::test::stub::StubRouter::new()
			.route(
				"/v1/chat/completions",
				http::Method::POST,
				upstream_fixtures::reply(move |_| {
					let calls = retry_calls.clone();
					async move {
						calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
						reinhardt::Response::new(http::StatusCode::INTERNAL_SERVER_ERROR)
							.with_json(&json!({"error":"retry fixture"}))
							.unwrap()
					}
				}),
			)
			.into_server_router(),
	)
}

/// Maintenance starts from committed rows, independently of live tool admission.
#[rstest::fixture]
async fn command_cleanup_fixture(
	#[default(1)] operation_count: usize,
	#[future(awt)]
	#[from(common::native_application)]
	application: common::ApplicationFixture,
	core_storage: Arc<tempfile::TempDir>,
) -> (CoreFixture, Uuid, Uuid) {
	let (mut f, url, schema) = application.runtime.parts();
	let app = application.application;
	let (policy, token, task) = bootstrap(&f, &app, "http://localhost:19999").await;
	let (status, response) = request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{task}/delegate"),
		json!({"node_id":f.config.deployment,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	let run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	let area = Uuid::new_v4();
	let id = Uuid::new_v4();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("core_areas"))
			.columns(
				[
					"id",
					"tenant",
					"home_node",
					"agent_id",
					"owner",
					"workspace_id",
					"thread_id",
					"state",
				]
				.map(Alias::new),
			)
			.values(vec![
				area.into(),
				"acme".into(),
				run.home_node.clone().into(),
				run.agent_id.clone().into(),
				"alice".into(),
				run.workspace_id.into(),
				Uuid::new_v4().into(),
				"running".into(),
			])
			.unwrap()
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	for index in 0..operation_count {
		let id = if index == 0 { id } else { Uuid::new_v4() };
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("core_operations"))
				.columns(
					[
						"id",
						"area_id",
						"run_id",
						"tenant",
						"principal",
						"credential_id",
						"subjects",
						"request_key",
						"digest",
						"kind",
						"state",
						"epoch",
						"generation",
						"revision",
						"policy_revision",
						"input",
						"result",
					]
					.map(Alias::new),
				)
				.values(vec![
					id.into(),
					area.into(),
					run.id.into(),
					"acme".into(),
					"alice".into(),
					Uuid::new_v4().into(),
					json!(["alice"]).into(),
					id.to_string().into(),
					"immutable-intent".into(),
					"shell".into(),
					"prepared".into(),
					1_i64.into(),
					1_i64.into(),
					1_i64.into(),
					1_i64.into(),
					json!({"command":"never replay","seconds":1}).into(),
					json!({}).into(),
				])
				.unwrap()
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	}
	let root = core_storage.path().to_owned();
	f.store.capabilities = Runtime::new(Profile {
		admission: true,
		storage: root.clone(),
		..Profile::default()
	})
	.unwrap();
	(
		CoreFixture {
			_storage: core_storage,
			http_client: reqwest::Client::new(),
			f,
			app,
			token,
			policy,
			task,
			url,
			schema,
			root,
		},
		area,
		id,
	)
}

/// A protocol stub: proves durable cleanup, not actual Kubernetes termination.
async fn cleanup_runner(
	c: &mut CoreFixture,
	operation: Uuid,
	uncertain: bool,
) -> (
	reinhardt::test::fixtures::server::TestServerGuard,
	Arc<std::sync::atomic::AtomicBool>,
	Arc<std::sync::Mutex<Vec<Value>>>,
	Arc<std::sync::Mutex<String>>,
) {
	use sha2::Digest as _;
	use std::sync::{
		Mutex,
		atomic::{AtomicBool, Ordering},
	};
	let confirmed = Arc::new(AtomicBool::new(false));
	let requests = Arc::new(Mutex::new(Vec::new()));
	let stopped = confirmed.clone();
	let observed = requests.clone();
	let instance = Arc::new(Mutex::new("cleanup-runner".to_owned()));
	let health_instance = instance.clone();
	let router = reinhardt::ServerRouter::new().handler(
		"/{*rest}",
		upstream_fixtures::any_handler(move |request: reinhardt::Request| {
			assert_eq!(
				request.headers.get("authorization").unwrap(),
				"Bearer aidash-command-cleanup-fixture",
				"cleanup fixtures require the token provisioned by scripts/test-rust.sh"
			);
			let path = request.uri.path().to_owned();
			let body = request.json::<Value>().unwrap_or(Value::Null);
			observed
				.lock()
				.unwrap()
				.push(json!({"path":path,"body":body}));
			let content = b"new";
			let file_digest = format!("{:x}", sha2::Sha256::digest(content));
			let mut output = json!({"operation_id":operation,"digest":"immutable-intent","status":"uncertain",
				"termination_confirmed":stopped.load(Ordering::SeqCst),"output_harvested":true,"acknowledged":false,
				"stdout":"cmVjb3ZlcmVkIHBhcnRpYWw=","files":[{"object_id":file_digest,"digest":file_digest,"size":3,"path":"saved.txt"}],"truncated":true});
			let proof = aidash_domain::capabilities::operations::processing::output_digest(&health_instance.lock().unwrap(), &output).unwrap();
			output["output_digest"] = json!(proof);
			let response = if path == "/v1/health" {
				json!({"instance":*health_instance.lock().unwrap()})
			} else if path.ends_with("/cancel") || path.ends_with("/stop") {
				json!({"operation_id":operation,"digest":"immutable-intent","termination_confirmed":stopped.load(Ordering::SeqCst)})
			} else if path == format!("/v1/operations/{operation}") && uncertain {
				output
			} else if path == format!("/v1/operations/{operation}/files/{file_digest}") && uncertain {
				json!({"data":"bmV3","offset":0,"size":3,"digest":file_digest})
			} else if path == format!("/v1/operations/{operation}/ack") {
				assert_eq!(body["digest"], "immutable-intent");
				if uncertain {
					assert!(stopped.load(Ordering::SeqCst));
					assert_eq!(body["output_digest"], proof, "uncertain ACK requires adopted output");
				}
				if uncertain {
					json!({"operation_id":operation,"digest":"immutable-intent","status":"uncertain","termination_confirmed":true,"acknowledged":true,"acknowledged_output_digest":proof})
				} else { json!({"acknowledged":true}) }
			} else {
				json!({"status":"absent"})
			};
			async move { reinhardt::Response::ok().with_json(&response).unwrap() }
		}),
	);
	let guard = upstream_fixtures::upstream(Arc::new(router)).await;
	let mut profile = (*c.f.store.capabilities.0).clone();
	profile.runner = Some(serde_json::from_value(json!({"endpoint":guard.url,"token_env":"AIDASH_COMMAND_CLEANUP_TEST_TOKEN","image":"fixture@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","runtime_class":"fixture","namespace":"fixture"})).unwrap());
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	(guard, confirmed, requests, instance)
}

async fn cleanup_operation(c: &CoreFixture, id: Uuid) -> (String, Value) {
	sqlx::query_as(
		&Query::select()
			.columns(["state", "result"].map(Alias::new))
			.from(Alias::new("core_operations"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(c.f.store.pool.fixture_driver())
	.await
	.unwrap()
}

#[rstest::rstest]
#[case::dispatched(true)]
#[case::never_dispatched(false)]
#[tokio::test]
async fn withdrawn_dispatched_receipt_is_durably_acknowledged_after_retry(
	#[case] dispatched: bool,
	#[future] command_cleanup_fixture: (CoreFixture, Uuid, Uuid),
	worker_control: WorkerControl,
) {
	use std::sync::atomic::Ordering;
	let (mut c, _, id) = Box::pin(command_cleanup_fixture).await;
	let (_runner, confirmed, requests, _) = cleanup_runner(&mut c, id, false).await;
	sqlx::query(
		&Query::update()
			.table(Alias::new("core_operations"))
			.value(
				Alias::new("state"),
				if dispatched { "submitted" } else { "prepared" },
			)
			.value(
				Alias::new("runner_instance"),
				if dispatched {
					Some("cleanup-runner")
				} else {
					None
				},
			)
			.value(
				Alias::new("result"),
				json!({"output":"retained output","offset":17}),
			)
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	let worker = CapabilityWorker {
		stop: worker_control.stop,
		handle: tokio::spawn(aidash_server::capabilities::operations::run(
			c.f.store.clone(),
			worker_control.receiver,
		)),
	};
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(12);
	loop {
		let (state, result) = cleanup_operation(&c, id).await;
		if !dispatched && state == "withdrawn" {
			break;
		}
		if state == "cancelling" {
			assert_eq!(result["termination_confirmed"], false);
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"withdrawal did not retry: {state} {result}"
		);
		tokio::time::sleep(std::time::Duration::from_millis(30)).await;
	}
	confirmed.store(true, Ordering::SeqCst);
	loop {
		let (state, result) = cleanup_operation(&c, id).await;
		if state == "withdrawn" && (!dispatched || result["runner_acknowledged"] == true) {
			assert_eq!(result["termination_confirmed"], true);
			assert_eq!(result["effects_may_have_occurred"], dispatched);
			if !dispatched {
				assert!(result.get("runner_acknowledged").is_none());
			}
			assert_eq!(result["output"], "retained output");
			assert_eq!(result["offset"], 17);
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"withdrawn receipt stranded: {state} {result}"
		);
		tokio::time::sleep(std::time::Duration::from_millis(30)).await;
	}
	assert_eq!(
		requests
			.lock()
			.unwrap()
			.iter()
			.any(|r| r["path"] == format!("/v1/operations/{id}/ack")),
		dispatched
	);
	worker.stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	if !dispatched {
		let requests = requests.lock().unwrap();
		// Independent allocation housekeeping does not dispatch an operation.
		assert!(
			requests.iter().all(|request| matches!(
				request["path"].as_str(),
				Some("/v1/health" | "/v1/allocations/receipts")
			)),
			"never-dispatched cleanup must only perform housekeeping: {requests:?}"
		);
	}
	c.close().await;
}

async fn python_receipt_snapshot(
	c: &CoreFixture,
	id: Uuid,
) -> (String, Value, Option<String>, i64, i64, String, Value) {
	sqlx::query_as(
		&Query::select()
			.columns(
				[
					"digest",
					"input",
					"runner_instance",
					"epoch",
					"generation",
					"state",
					"result",
				]
				.map(Alias::new),
			)
			.from(Alias::new("core_operations"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(c.f.store.pool.fixture_driver())
	.await
	.unwrap()
}

async fn python_record_snapshot(c: &CoreFixture, area: Uuid) -> (String, Value) {
	sqlx::query_as(
		&Query::select()
			.columns(["state", "data"].map(Alias::new))
			.from(Alias::new("core_records"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(area)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(c.f.store.pool.fixture_driver())
	.await
	.unwrap()
}

#[rstest::rstest]
#[tokio::test]
async fn frozen_python_stop_persists_all_bound_cells_without_touching_replacements(
	#[future] command_cleanup_fixture: (CoreFixture, Uuid, Uuid),
	worker_control: WorkerControl,
) {
	use std::sync::atomic::Ordering;
	let (mut c, area, latest) = Box::pin(command_cleanup_fixture).await;
	let (_runner, confirmed, requests, instance) = cleanup_runner(&mut c, latest, false).await;
	let session = Uuid::new_v4();
	let other_session = Uuid::new_v4();
	let original = json!({"termination_confirmed":false,"runner_acknowledged":true,"writer_frozen":true,"stdout":"saved output","displays":[{"type":"text","data":"kept"}],"offset":19,"error":null});
	sqlx::query(
		&Query::update()
			.table(Alias::new("core_operations"))
			.value(Alias::new("state"), "completed")
			.value(Alias::new("kind"), "code_interpreter")
			.value(Alias::new("epoch"), 3_i64)
			.value(Alias::new("runner_instance"), "cleanup-runner")
			.value(
				Alias::new("input"),
				json!({"session_id":session,"command":"do not replay"}),
			)
			.value(Alias::new("result"), original.clone())
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(latest)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	let mut rows = vec![(latest, true)];
	for (epoch, generation, instance, sid, state, stopped) in [
		(1_i64, 1_i64, "cleanup-runner", session, "completed", true),
		(2, 1, "cleanup-runner", session, "failed", true),
		(4, 1, "cleanup-runner", session, "completed", false),
		(2, 2, "cleanup-runner", session, "completed", false),
		(2, 1, "replacement-runner", session, "completed", false),
		(2, 1, "cleanup-runner", other_session, "completed", false),
	] {
		let id = Uuid::new_v4();
		let columns = [
			"id",
			"area_id",
			"run_id",
			"tenant",
			"principal",
			"credential_id",
			"subjects",
			"request_key",
			"digest",
			"kind",
			"state",
			"epoch",
			"generation",
			"revision",
			"policy_revision",
			"input",
			"result",
			"runner_instance",
		];
		let mut select = Query::select();
		for column in columns {
			select.expr(match column {
				"id" => Expr::value(id),
				"request_key" => Expr::value(id.to_string()),
				"epoch" => Expr::value(epoch),
				"generation" => Expr::value(generation),
				"runner_instance" => Expr::value(instance),
				"state" => Expr::value(state),
				"input" => Expr::value(json!({"session_id":sid,"command":"do not replay"})),
				_ => Expr::col(Alias::new(column)),
			});
		}
		select
			.from(Alias::new("core_operations"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(latest)));
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("core_operations"))
				.columns(columns.map(Alias::new))
				.from_subquery(select)
				.to_string(PostgresQueryBuilder),
		)
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
		rows.push((id, stopped));
	}
	let binding = json!({"operation_id":latest,"session_id":session,"instance":"cleanup-runner","last_used":chrono::Utc::now(),"last_revision":1});
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("core_records"))
			.columns(["id", "tenant", "owner", "area_id", "kind", "state", "data"].map(Alias::new))
			.values(vec![
				area.into(),
				"acme".into(),
				"alice".into(),
				area.into(),
				"python_session".into(),
				"frozen".into(),
				binding.clone().into(),
			])
			.unwrap()
			.to_string(PostgresQueryBuilder),
	)
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	let mut before = Vec::new();
	for (id, _) in &rows {
		before.push(python_receipt_snapshot(&c, *id).await);
	}
	let worker = CapabilityWorker {
		stop: worker_control.stop,
		handle: tokio::spawn(aidash_server::capabilities::operations::run(
			c.f.store.clone(),
			worker_control.receiver,
		)),
	};
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
	loop {
		if requests
			.lock()
			.unwrap()
			.iter()
			.any(|r| r["path"] == format!("/v1/sessions/{session}/stop"))
		{
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"revoked session was not reaped"
		);
		tokio::time::sleep(std::time::Duration::from_millis(30)).await;
	}
	assert_eq!(
		python_record_snapshot(&c, area).await,
		("frozen".into(), binding)
	);
	for ((id, _), snapshot) in rows.iter().zip(&before) {
		assert_eq!(&python_receipt_snapshot(&c, *id).await, snapshot);
	}
	*instance.lock().unwrap() = "replacement-runner".into();
	confirmed.store(true, Ordering::SeqCst);
	loop {
		if requests
			.lock()
			.unwrap()
			.iter()
			.any(|r| r["path"] == "/v1/health")
		{
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"stop identity was not checked"
		);
		tokio::time::sleep(std::time::Duration::from_millis(30)).await;
	}
	assert_eq!(python_record_snapshot(&c, area).await.0, "frozen");
	for ((id, _), snapshot) in rows.iter().zip(&before) {
		assert_eq!(&python_receipt_snapshot(&c, *id).await, snapshot);
	}
	*instance.lock().unwrap() = "cleanup-runner".into();
	loop {
		if python_record_snapshot(&c, area).await.0 == "reset" {
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"confirmed session remained frozen"
		);
		tokio::time::sleep(std::time::Duration::from_millis(30)).await;
	}
	for ((id, stopped), mut expected) in rows.into_iter().zip(before) {
		if stopped {
			expected.6["termination_confirmed"] = json!(true);
		}
		assert_eq!(
			python_receipt_snapshot(&c, id).await,
			expected,
			"exact cell stop proof for {id}"
		);
	}
	worker.stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}

/// Protocol fixture exercises the production stop owner and durable retries;
/// physical Kubernetes termination remains covered by the runner's own tests.
async fn delete_and_restore_owning_node(c: &CoreFixture) {
	let node = common::default_node(&c.f.store, "acme").await;
	let sibling = common::second_node_in(c.f.store.pool.fixture_driver(), "acme").await;
	for (path, body) in [
		(
			format!("/api/tenants/acme/nodes/{sibling}/default"),
			Value::Null,
		),
		(
			format!("/api/tenants/acme/nodes/{}/deletion", node.node),
			json!({"confirmation":node.node}),
		),
		(
			format!("/api/tenants/acme/nodes/{}/restore", node.node),
			Value::Null,
		),
	] {
		let (status, body) = request(&c.app, &c.f.config.api_token, "POST", &path, body).await;
		assert_eq!(status, 200, "{body}");
	}
}

async fn retry_node_cancellations(c: &CoreFixture) {
	sqlx::query(
		&Query::update()
			.table(Alias::new("node_cancellation_jobs"))
			.value_expr(Alias::new("retry_at"), Expr::current_timestamp())
			.to_string(PostgresQueryBuilder),
	)
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	aidash_server::generation::provision::reconcile(&c.f.clone())
		.await
		.unwrap();
}

#[rstest::rstest]
#[case::prepared("prepared")]
#[case::running("running")]
#[case::completed_during_deletion("completed")]
#[tokio::test]
async fn node_deletion_captures_running_python_until_its_original_writer_is_stopped(
	#[case] state: &str,
	#[future] command_cleanup_fixture: (CoreFixture, Uuid, Uuid),
	worker_control: WorkerControl,
) {
	use std::sync::atomic::Ordering;
	let (mut c, area, id) = Box::pin(command_cleanup_fixture).await;
	let (_runner, confirmed, requests, instance) = cleanup_runner(&mut c, id, false).await;
	let session = Uuid::new_v4();
	let active = if state == "completed" {
		"running"
	} else {
		state
	};
	sqlx::query(
		&Query::update()
			.table(Alias::new("core_operations"))
			.value(Alias::new("state"), active)
			.value(Alias::new("kind"), "code_interpreter")
			.value(
				Alias::new("runner_instance"),
				(active == "running").then_some("cleanup-runner"),
			)
			.value(
				Alias::new("input"),
				json!({"session_id":session,"code":"must never replay"}),
			)
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	let binding = json!({"operation_id":id,"session_id":session,"instance":"cleanup-runner","last_used":chrono::Utc::now(),"last_revision":1});
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("core_records"))
			.columns(["id", "tenant", "owner", "area_id", "kind", "state", "data"].map(Alias::new))
			.values(vec![
				area.into(),
				"acme".into(),
				"alice".into(),
				area.into(),
				"python_session".into(),
				"running".into(),
				binding.clone().into(),
			])
			.unwrap()
			.to_string(PostgresQueryBuilder),
	)
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	delete_and_restore_owning_node(&c).await;
	let captured: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(reinhardt::query::Func::count(Expr::col("id").into()))
			.from(Alias::new("node_cancellation_jobs"))
			.and_where(Expr::col("kind").eq("python"))
			.and_where(Expr::col("work_id").eq(Expr::value(area)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(
		captured, 1,
		"a running heap needs its own durable cleanup job"
	);
	let heap_state = if state == "completed" {
		// Ordinary completion may settle the cell after deletion captured its
		// running binding. It must not strand the unchanged heap's cleanup job.
		sqlx::query(
			&Query::update()
				.table(Alias::new("core_operations"))
				.value(Alias::new("state"), "completed")
				.value(
					Alias::new("result"),
					json!({"termination_confirmed":false,"runner_acknowledged":true,"writer_frozen":true}),
				)
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
		sqlx::query(
			&Query::update()
				.table(Alias::new("core_records"))
				.value(Alias::new("state"), "frozen")
				.value(Alias::new("revision"), 1_i64)
				.and_where(Expr::col("id").eq(Expr::value(area)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
		"frozen"
	} else {
		"running"
	};
	retry_node_cancellations(&c).await;
	assert_eq!(
		python_record_snapshot(&c, area).await,
		(heap_state.into(), binding.clone())
	);
	*instance.lock().unwrap() = "replacement-runner".into();
	confirmed.store(true, Ordering::SeqCst);
	retry_node_cancellations(&c).await;
	assert_eq!(
		python_record_snapshot(&c, area).await,
		(heap_state.into(), binding.clone())
	);
	*instance.lock().unwrap() = "cleanup-runner".into();
	// Only the dispatched cell needs the receipt worker. Keep the completed
	// case owned by deletion rather than racing unrelated idle-heap maintenance.
	let worker = (state == "running").then(|| CapabilityWorker {
		stop: worker_control.stop,
		handle: tokio::spawn(aidash_server::capabilities::operations::run(
			c.f.store.clone(),
			worker_control.receiver,
		)),
	});
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(12);
	loop {
		retry_node_cancellations(&c).await;
		let pending: i64 = sqlx::query_scalar(
			&Query::select()
				.expr(reinhardt::query::Func::count(Expr::col("id").into()))
				.from(Alias::new("node_cancellation_jobs"))
				.and_where(Expr::col("work_id").is_in([Expr::value(id), Expr::value(area)]))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
		if python_record_snapshot(&c, area).await.0 == "reset" && pending == 0 {
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"the stopped heap or {pending} cancellation jobs still block Node purge"
		);
		tokio::time::sleep(std::time::Duration::from_millis(30)).await;
	}
	let (saved, result) = cleanup_operation(&c, id).await;
	assert_eq!(
		saved,
		if state == "completed" {
			"completed"
		} else {
			"withdrawn"
		}
	);
	assert_eq!(result["termination_confirmed"], true);
	assert_eq!(
		python_record_snapshot(&c, area).await.1["reset_reason"],
		"owning_node_deleted"
	);
	assert!(
		requests
			.lock()
			.unwrap()
			.iter()
			.any(|r| r["path"] == format!("/v1/sessions/{session}/stop"))
	);
	if let Some(worker) = worker {
		worker.stop.send(true).unwrap();
		worker.await.unwrap().unwrap();
	}
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn stopped_uncertain_output_without_an_ack_flag_is_adopted_and_acknowledged(
	#[future] command_cleanup_fixture: (CoreFixture, Uuid, Uuid),
	worker_control: WorkerControl,
) {
	use std::sync::atomic::Ordering;
	let (mut c, _, id) = Box::pin(command_cleanup_fixture).await;
	let (_runner, confirmed, requests, _) = cleanup_runner(&mut c, id, true).await;
	confirmed.store(true, Ordering::SeqCst);
	// This is the durable result emitted by ordinary uncertain reconciliation.
	// No Node deletion subsequently adds an acknowledgement flag for it.
	sqlx::query(
		&Query::update()
			.table(Alias::new("core_operations"))
			.value(Alias::new("state"), "uncertain")
			.value(Alias::new("runner_instance"), "cleanup-runner")
			.value(
				Alias::new("result"),
				json!({"error":"sandbox lost","termination_confirmed":true}),
			)
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	let worker = CapabilityWorker {
		stop: worker_control.stop,
		handle: tokio::spawn(aidash_server::capabilities::operations::run(
			c.f.store.clone(),
			worker_control.receiver,
		)),
	};
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
	loop {
		let (state, result) = cleanup_operation(&c, id).await;
		assert_eq!(state, "uncertain");
		assert_eq!(result["error"], "sandbox lost");
		if result["runner_acknowledged"] == true {
			let output = &result["stopped_output"];
			for (file, bytes) in [
				(&output["files"][0], b"new".as_slice()),
				(&output["output_file"], b"recovered partial".as_slice()),
			] {
				let file: Uuid = serde_json::from_value(file["file_id"].clone()).unwrap();
				assert_eq!(
					tokio::fs::read(c.root.join(file.simple().to_string()))
						.await
						.unwrap(),
					bytes
				);
			}
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"ordinary uncertain output was excluded from receipt adoption: {result}"
		);
		tokio::time::sleep(std::time::Duration::from_millis(30)).await;
	}
	assert!(
		requests
			.lock()
			.unwrap()
			.iter()
			.any(|r| r["path"] == format!("/v1/operations/{id}/ack"))
	);
	worker.stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}

#[rstest::rstest]
#[case::completed("completed", true)]
#[case::uncertain("uncertain", true)]
#[case::uncertain_pending_ack("uncertain", false)]
#[tokio::test]
async fn node_deletion_retries_original_terminal_writer_without_rewriting_output(
	#[case] state: &str,
	#[case] acknowledged: bool,
	#[future] command_cleanup_fixture: (CoreFixture, Uuid, Uuid),
	worker_control: WorkerControl,
) {
	use std::sync::atomic::Ordering;
	let (mut c, _, id) = Box::pin(command_cleanup_fixture).await;
	let (_runner, confirmed, requests, instance) =
		cleanup_runner(&mut c, id, state == "uncertain" && !acknowledged).await;
	let original = json!({"termination_confirmed":false,"runner_acknowledged":acknowledged,"effects_may_have_occurred":true,"stdout":"retained output","error":"saved outcome","offset":17});
	sqlx::query(
		&Query::update()
			.table(Alias::new("core_operations"))
			.value(Alias::new("state"), state)
			.value(Alias::new("runner_instance"), "cleanup-runner")
			.value(Alias::new("result"), original.clone())
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	delete_and_restore_owning_node(&c).await;
	for (runner, stopped, done) in [
		("replacement", true, false),
		("cleanup-runner", false, false),
		("cleanup-runner", true, true),
	] {
		*instance.lock().unwrap() = runner.into();
		confirmed.store(stopped, Ordering::SeqCst);
		sqlx::query(
			&Query::update()
				.table(Alias::new("node_cancellation_jobs"))
				.value_expr(Alias::new("retry_at"), Expr::current_timestamp())
				.and_where(Expr::col("work_id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
		// A new dispatcher invocation reads durable due state; it has no volatile cursor.
		aidash_server::generation::provision::reconcile(&c.f.clone())
			.await
			.unwrap();
		let (saved, result) = cleanup_operation(&c, id).await;
		assert_eq!(saved, state);
		let mut expected = original.clone();
		expected["termination_confirmed"] = json!(done);
		assert_eq!(result, expected);
		let pending: i64 = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("node_cancellation_jobs"))
				.and_where(Expr::col("kind").eq("operation"))
				.and_where(Expr::col("work_id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
		assert_eq!(pending, if done && acknowledged { 0 } else { 1 });
	}
	if !acknowledged {
		let profile = (*c.f.store.capabilities.0).clone();
		let mut constrained = profile.clone();
		constrained.working_bytes = 4;
		constrained.retained_bytes = 4;
		c.f.store.capabilities = Runtime::new(constrained).unwrap();
		let (stop, receiver) = tokio::sync::watch::channel(false);
		let failed = CapabilityWorker {
			stop,
			handle: tokio::spawn(aidash_server::capabilities::operations::run(
				c.f.store.clone(),
				receiver,
			)),
		};
		let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(12);
		loop {
			let reads = requests
				.lock()
				.unwrap()
				.iter()
				.filter(|r| r["path"].as_str().is_some_and(|p| p.contains("/files/")))
				.count();
			if reads >= 2 {
				break;
			}
			assert!(
				tokio::time::Instant::now() < deadline,
				"saved output was not retried after quota failure"
			);
			tokio::time::sleep(std::time::Duration::from_millis(30)).await;
		}
		failed.stop.send(true).unwrap();
		failed.await.unwrap().unwrap();
		assert_eq!(
			cleanup_operation(&c, id).await.1["runner_acknowledged"],
			false
		);
		assert!(
			!requests
				.lock()
				.unwrap()
				.iter()
				.any(|r| r["path"] == format!("/v1/operations/{id}/ack"))
		);
		let objects: i64 = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("core_objects"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
		assert_eq!(
			objects, 0,
			"failed adoption rolls back objects and quota together"
		);
		c.f.store.capabilities = Runtime::new(profile).unwrap();
	}
	if !acknowledged {
		// Stop proof alone cannot retire the queue. Let the production receipt
		// owner acknowledge this exact saved operation before maintenance retries.
		let worker = CapabilityWorker {
			stop: worker_control.stop,
			handle: tokio::spawn(aidash_server::capabilities::operations::run(
				c.f.store.clone(),
				worker_control.receiver,
			)),
		};
		let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(12);
		loop {
			let (saved, result) = cleanup_operation(&c, id).await;
			assert_eq!(saved, state, "ACK must preserve the semantic outcome");
			if result["runner_acknowledged"] == true {
				let mut expected = original.clone();
				expected["termination_confirmed"] = json!(true);
				expected["runner_acknowledged"] = json!(true);
				let output = &result["stopped_output"];
				assert!(result["stopped_output_digest"].is_string());
				for (file, bytes) in [
					(&output["files"][0], b"new".as_slice()),
					(&output["output_file"], b"recovered partial".as_slice()),
				] {
					let id = file["file_id"].as_str().unwrap().parse::<Uuid>().unwrap();
					assert_eq!(
						tokio::fs::read(c.root.join(id.simple().to_string()))
							.await
							.unwrap(),
						bytes
					);
				}
				let mut semantic = result.clone();
				semantic.as_object_mut().unwrap().remove("stopped_output");
				semantic
					.as_object_mut()
					.unwrap()
					.remove("stopped_output_digest");
				assert_eq!(semantic, expected);
				break;
			}
			assert!(
				tokio::time::Instant::now() < deadline,
				"physically stopped uncertain receipt was not acknowledged: {result}"
			);
			tokio::time::sleep(std::time::Duration::from_millis(30)).await;
		}
		worker.stop.send(true).unwrap();
		worker.await.unwrap().unwrap();
		sqlx::query(
			&Query::update()
				.table(Alias::new("node_cancellation_jobs"))
				.value_expr(Alias::new("retry_at"), Expr::current_timestamp())
				.and_where(Expr::col("work_id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
		aidash_server::generation::provision::reconcile(&c.f.clone())
			.await
			.unwrap();
		let pending: i64 = sqlx::query_scalar(
			&Query::select()
				.expr(reinhardt::query::Func::count(Expr::col("id").into()))
				.from(Alias::new("node_cancellation_jobs"))
				.and_where(Expr::col("kind").eq("operation"))
				.and_where(Expr::col("work_id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
		assert_eq!(pending, 0, "positive stop and owner ACK retire the job");
	}
	assert_eq!(
		requests
			.lock()
			.unwrap()
			.iter()
			.filter(|request| request["path"] == format!("/v1/operations/{id}/cancel"))
			.count(),
		2,
		"replacement health must never receive the old stop"
	);
	cleanup(c.f, &c.url, &c.schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn node_deletion_due_cursor_visits_later_jobs_after_a_failed_page(
	#[future]
	#[with(35)]
	command_cleanup_fixture: (CoreFixture, Uuid, Uuid),
) {
	let (mut c, area, id) = Box::pin(command_cleanup_fixture).await;
	let (_runner, _, requests, instance) = cleanup_runner(&mut c, id, false).await;
	*instance.lock().unwrap() = "replacement".into();
	sqlx::query(&Query::update().table(Alias::new("core_operations"))
 .value(Alias::new("state"),"uncertain").value(Alias::new("runner_instance"),"cleanup-runner")
 .value(Alias::new("result"),json!({"termination_confirmed":false,"runner_acknowledged":true,"effects_may_have_occurred":true}))
 .and_where(Expr::col("area_id").eq(Expr::value(area))).to_string(PostgresQueryBuilder)).execute(c.f.store.pool.fixture_driver()).await.unwrap();
	let node = common::default_node(&c.f.store, "acme").await;
	let sibling = common::second_node_in(c.f.store.pool.fixture_driver(), "acme").await;
	for (path, body) in [
		(
			format!("/api/tenants/acme/nodes/{sibling}/default"),
			Value::Null,
		),
		(
			format!("/api/tenants/acme/nodes/{}/deletion", node.node),
			json!({"confirmation":node.node}),
		),
		(
			format!("/api/tenants/acme/nodes/{}/restore", node.node),
			Value::Null,
		),
	] {
		let (status, body) = request(&c.app, &c.f.config.api_token, "POST", &path, body).await;
		assert_eq!(status, 200, "{body}");
	}
	aidash_server::generation::provision::reconcile(&c.f)
		.await
		.unwrap();
	assert_eq!(
		requests.lock().unwrap().len(),
		32,
		"each selected operation stops at mismatched health"
	);
	let due = || {
		Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("node_cancellation_jobs"))
			.and_where(Expr::col("kind").eq("operation"))
			.and_where(Expr::col("retry_at").lte(Expr::current_timestamp()))
			.to_string(PostgresQueryBuilder)
	};
	assert_eq!(
		sqlx::query_scalar::<_, i64>(&due())
			.fetch_one(c.f.store.pool.fixture_driver())
			.await
			.unwrap(),
		3
	);
	aidash_server::generation::provision::reconcile(&c.f.clone())
		.await
		.unwrap();
	assert_eq!(
		requests.lock().unwrap().len(),
		35,
		"failed first page must not starve the later page"
	);
	assert_eq!(
		sqlx::query_scalar::<_, i64>(&due())
			.fetch_one(c.f.store.pool.fixture_driver())
			.await
			.unwrap(),
		0
	);
	cleanup(c.f, &c.url, &c.schema).await;
}
