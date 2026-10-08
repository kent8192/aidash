use super::*;
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
	.execute(c.f.store.pool.driver())
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
		.execute(c.f.store.pool.driver())
		.await
	}
	.unwrap();
	(c, run, area, id)
}

#[rstest::rstest]
#[tokio::test]
async fn authority_withdrawal_before_dispatch_keeps_saved_files_usable(
	#[future] withdrawn_operation_fixture: (CoreFixture, aidash_server::domain::Run, Value, Uuid),
) {
	let (c, run, area, id) = Box::pin(withdrawn_operation_fixture).await;
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash_server::capabilities::operations::run(
		c.f.store.clone(),
		rx,
	));
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
			.fetch_one(c.f.store.pool.driver())
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
		.fetch_one(c.f.store.pool.driver())
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
	.fetch_all(c.f.store.pool.driver())
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
	c.app = common::application(c.f.clone()).await;
	(c, last)
}

#[rstest::rstest]
#[tokio::test]
async fn stale_extraction_credentials_do_not_starve_later_uploads(
	#[future] extraction_queue_fixture: (CoreFixture, String),
) {
	let (c, path) = Box::pin(extraction_queue_fixture).await;
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash_server::capabilities::operations::run(
		c.f.store.clone(),
		rx,
	));
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
		.fetch_one(c.f.store.pool.driver())
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
	c.app = common::application(c.f.clone()).await;
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
	c.app = common::application(c.f.clone()).await;
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
	.fetch_one(c.f.store.pool.driver())
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

#[rstest::rstest]
#[tokio::test]
async fn cached_skill_context_rechecks_descriptor_authority_before_model_retry(
	#[future] test_environment: Arc<TestEnvironment>,
) {
	use std::sync::atomic::{AtomicUsize, Ordering};
	let calls = Arc::new(AtomicUsize::new(0));
	let seen = calls.clone();
	let model = axum::Router::new().route(
		"/v1/chat/completions",
		axum::routing::post(move || {
			let seen = seen.clone();
			async move {
				seen.fetch_add(1, Ordering::SeqCst);
				(
					http::StatusCode::INTERNAL_SERVER_ERROR,
					axum::Json(json!({"error":"retry fixture"})),
				)
			}
		}),
	);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, model).await.unwrap() });
	let c =
		build_core_fixture_at(test_environment.await, "aidash://execution-test", &endpoint).await;
	let admitted = admit(&c).await;
	let harness = aidash_server::harness::Harness {
		federation: c.f.clone(),
	};
	harness.worker_once().await.unwrap();
	harness.worker_once().await.unwrap();
	assert_eq!(calls.load(Ordering::SeqCst), 1);
	let retry = c.f.store.run(admitted.id).await.unwrap();
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
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"withdraw-skill-list","effect":"deny","subjects":{"any":true},"actions":["tool.invoke"],"resources":{"kinds":["tool"],"ids":[aidash_domain::registry::bindings::QualifiedRef::builtin(&c.f.config.node_id,"skill_list").resource_id()]}}));
	let (status, result) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
	let blocked = loop {
		harness.worker_once().await.unwrap();
		let current = c.f.store.run(admitted.id).await.unwrap();
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
	assert_eq!(blocked.error.as_deref(), Some("execution authority denied"));
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
	server.abort();
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
	let (status, result) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
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
	.fetch_one(c.f.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(
		count, 0,
		"withdrawn binding must not create a Host operation"
	);
	c.close().await;
}
