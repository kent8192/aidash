use super::*;
#[path = "../support/isolated.rs"]
mod isolated;

#[rstest::fixture]
fn wait_router(#[from(upstream_fixtures::hits)] calls: Arc<AtomicUsize>) -> Arc<Router> {
	Arc::new(reinhardt::test::stub::StubRouter::new().route(
		"/v1/chat/completions", http::Method::POST, reply(move |_request: reinhardt::Request| {
			let calls = calls.clone();
			async move {
				let n = calls.fetch_add(1, Ordering::SeqCst);
				let message = if n.is_multiple_of(2) {
					json!({"role":"assistant","content":null,"tool_calls":[{"id":"wait","type":"function","function":{"name":"workspace_wait","arguments":json!({"seconds":if n == 6 {5} else {300}}).to_string()}}]})
				} else { json!({"role":"assistant","content":"Awake"}) };
				reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":if n.is_multiple_of(2) {"tool_calls"} else {"stop"},"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
			}
		})
	).into_server_router())
}

async fn waiting(f: &Federation, run: Uuid) {
	let start = Instant::now();
	loop {
		let current = f.store.run(&TEST_FIXTURE, run).await.unwrap();
		if matches!(current.state, aidash_server::domain::RunState::Waiting(ref w) if matches!(w.as_ref(), aidash_server::domain::WaitingState::Workspace { .. }))
		{
			return;
		}
		assert!(
			start.elapsed() < Duration::from_secs(30),
			"not waiting: {current:?}; activations: {}",
			activations(f, run).await
		);
		tokio::time::sleep(Duration::from_millis(25)).await;
	}
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_wait_wakes_for_child_input_and_resume(
	#[from(activation_runtime)] _runtime: common::RuntimeFuture,
	#[from(upstream_fixtures::hits)] calls: Arc<AtomicUsize>,
	#[from(wait_router)]
	#[with(calls.clone())]
	_router: Arc<Router>,
	#[from(prepared_activation)]
	#[with("waits", _runtime.clone(), _router.clone(), None)]
	prepared: PreparedActivationFuture,
	#[future(awt)]
	#[from(initial_process)]
	#[with(prepared.clone(), "serve", 0, true, false)]
	process: Process,
) {
	let mut process = process;
	let prepared = prepared.await;
	let (f, url, schema) = prepared.runtime.parts();
	let token = prepared.token.clone();
	let broker = prepared.broker.clone();
	let provider = prepared.provider.clone();
	process.ready().await;
	for scenario in ["child", "input", "paused", "ceiling"] {
		let task = if scenario == "child" {
			prepared.task
		} else {
			fresh_task(&f, &token).await
		};
		let run = admit(&f, &token, task).await;
		waiting(&f, run).await;
		let before = calls.load(Ordering::SeqCst);
		let parent = f.store.task(&TEST_FIXTURE, task).await.unwrap();
		if scenario == "ceiling" {
			assert_eq!(
				calls.load(Ordering::SeqCst),
				before,
				"ceiling wait must not call the model early"
			);
		} else if scenario == "input" {
			post_json(
				&f,
				&token,
				&format!("/api/runs/{run}/message"),
				json!({"content":"New instruction","idempotency_key":Uuid::new_v4()}),
			)
			.await;
		} else {
			let child = post_json(
				&f,
				&token,
				&format!("/api/workspaces/{}/tasks", parent.workspace_id),
				json!({"title":"Child","description":"Wait fixture","parent_id":task}),
			)
			.await;
			let child = Uuid::parse_str(child["id"].as_str().unwrap()).unwrap();
			let db = f.store.pool.fixture_driver();
			sqlx::query("INSERT INTO authorization_task_origins (task_id, tenant, source_run_id, root_subject, subject_chain) VALUES ($1, 'acme', $2, 'alice', ARRAY['alice','fixture-agent'])").bind(child).bind(run).execute(db).await.unwrap();
			sqlx::query("UPDATE tasks SET status = 'RUNNING', owner = 'alice' WHERE id = $1")
				.bind(child)
				.execute(db)
				.await
				.unwrap();
			tokio::time::sleep(Duration::from_secs(2)).await;
			assert_eq!(
				calls.load(Ordering::SeqCst),
				before,
				"starting a child must not wake the parent"
			);
			if scenario == "paused" {
				sqlx::query("UPDATE runs SET control = 'PAUSED' WHERE id = $1")
					.bind(run)
					.execute(db)
					.await
					.unwrap();
			}
			sqlx::query("UPDATE tasks SET status = 'COMPLETED' WHERE id = $1")
				.bind(child)
				.execute(db)
				.await
				.unwrap();
			if scenario == "paused" {
				tokio::time::sleep(Duration::from_secs(2)).await;
				assert_eq!(
					calls.load(Ordering::SeqCst),
					before,
					"paused Run must remain asleep"
				);
				sqlx::query("UPDATE runs SET control = 'ACTIVE' WHERE id = $1")
					.bind(run)
					.execute(db)
					.await
					.unwrap();
			}
		}
		complete(&f, run).await;
		assert_eq!(
			calls.load(Ordering::SeqCst),
			before + 1,
			"{scenario} should wake once"
		);
	}
	process.stop();
	drop(process);
	drop(provider);
	broker
		.context
		.delete_stream(&broker.stream_name)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::fixture]
fn operation_id() -> Uuid {
	Uuid::new_v4()
}

#[rstest::fixture]
fn operation_router(
	#[from(upstream_fixtures::hits)] calls: Arc<AtomicUsize>,
	#[from(operation_id)] id: Uuid,
) -> Arc<Router> {
	Arc::new(reinhardt::test::stub::StubRouter::new().route("/v1/chat/completions", http::Method::POST, reply(move |_request: reinhardt::Request| {
		let calls = calls.clone();
		async move {
			let first = calls.fetch_add(1, Ordering::SeqCst) == 0;
			let message = if first { json!({"role":"assistant","content":null,"tool_calls":[{"id":"poll","type":"function","function":{"name":"shell_poll","arguments":json!({"operation_id":id,"wait_seconds":300}).to_string()}}]}) } else { json!({"role":"assistant","content":"Operation finished"}) };
			reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":if first {"tool_calls"} else {"stop"},"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
		}
	})).into_server_router())
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn operation_completion_wakes_worker_without_intermediate_model_call(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
	#[from(upstream_fixtures::hits)] calls: Arc<AtomicUsize>,
	#[from(operation_id)] operation: Uuid,
	#[from(operation_router)]
	#[with(calls.clone(), operation)]
	_router: Arc<Router>,
	#[from(upstream_fixtures::provider_transport)]
	#[with(_router.clone())]
	provider: upstream_fixtures::UpstreamFuture,
	#[from(common::native_application)]
	#[with(Default::default(), aidash_server::sse::Service::new(Default::default()), Arc::new(|router| router), runtime.clone())]
	fixture: common::ApplicationFuture,
) {
	let mut owned_profile = None;
	let profile = if std::env::var_os("AIDASH_TEST_CHILD").is_some() {
		std::env::var("AIDASH_CAPABILITY_PROFILE").unwrap()
	} else {
		let directory = tempfile::tempdir().unwrap();
		let path = directory.path().join("profile.json");
		std::fs::write(&path, serde_json::to_vec(&json!({"admission":true,"storage":directory.path().join("storage"),"runner":{"endpoint":"http://127.0.0.1:1","token_env":"AIDASH_COMMAND_CLEANUP_TEST_TOKEN","image":format!("sandbox@sha256:{}", "a".repeat(64)),"runtime_class":"fixture","namespace":"fixture"}})).unwrap()).unwrap();
		let path = path.to_string_lossy().into_owned();
		owned_profile = Some(directory);
		path
	};
	if !isolated::isolated_process(&[
		("AIDASH_CAPABILITY_PROFILE", profile.as_str()),
		(
			"AIDASH_COMMAND_CLEANUP_TEST_TOKEN",
			"aidash-command-cleanup-fixture",
		),
	])
	.await
	{
		return;
	}
	let _owned_profile = owned_profile;
	let runtime = runtime.await;
	let provider = provider.await;
	let fixture = fixture.await;
	let (f, url, schema) = runtime.parts();
	let app = fixture.application;
	let (mut policy, token, task) = bootstrap_with_area(&f, &app, &provider.url).await;
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let mut agent = f.registry.get(&tenant, "research", "1.0.0").await.unwrap();
	agent.version = "1.1.0".into();
	agent.binding_normalization = None;
	let offered = f
		.registry
		.list(
			&tenant,
			&aidash_server::registry::Search {
				kind: Some("bundle".into()),
				..Default::default()
			},
		)
		.await
		.unwrap()
		.into_iter()
		.filter(|bundle| bundle.tags.iter().any(|tag| tag == "sandboxed"))
		.collect::<Vec<_>>();
	assert!(
		offered
			.iter()
			.any(|bundle| bundle.config["members"].to_string().contains("shell_poll")),
		"offered bundles: {:?}",
		offered.iter().map(|bundle| &bundle.id).collect::<Vec<_>>()
	);
	let approvals = offered
		.iter()
		.flat_map(|bundle| {
			std::iter::once(json!({"id":bundle.id,"version":bundle.version})).chain(
				bundle.config["members"]
					.as_array()
					.unwrap()
					.iter()
					.map(|member| json!({"id":member["id"],"version":member["version"]})),
			)
		})
		.map(|entry| json!({"entry":entry,"enabled":true,"expected_revision":0}))
		.collect::<Vec<_>>();
	let (status, body) = request(
		&app,
		&token,
		"POST",
		"/api/authorization/acme/catalog/approvals",
		json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"approvals":approvals}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	agent.config["bindings"].as_array_mut().unwrap().extend(offered.iter().map(|bundle| json!({"kind":"bundle","target":{"registry_deployment":f.config.deployment,"id":bundle.id,"version":bundle.version},"narrow":{}})));
	let (status, body) = request(&app, &token, "POST", "/api/registry", json!(agent)).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(&app, &token, "POST", "/api/authorization/acme/catalog", json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"research","version":"1.1.0"},"expected_revision":0,"enabled":true})).await;
	assert_eq!(status, 200, "{body}");
	policy["subjects"][aidash_domain::agent_subject(
		&default_node(&f.store, "acme").await.to_string(),
		"research",
		"1.1.0",
	)] = json!({"kind":"agent","node":aidash_domain::nodes::NodeName::derived("acme")});
	let policy_revision: i64 =
		sqlx::query_scalar("SELECT revision FROM authorization_bundles WHERE tenant = 'acme'")
			.fetch_one(f.store.pool.fixture_driver())
			.await
			.unwrap();
	let (status, body) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":policy_revision,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{task}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.1.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let run = f
		.store
		.runs(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.task_id == task)
		.unwrap();
	let db = f.store.pool.fixture_driver();
	let credential: Uuid = sqlx::query_scalar("SELECT id FROM authorization_credentials WHERE tenant = 'acme' AND subject = 'alice' ORDER BY created_at DESC LIMIT 1").fetch_one(db).await.unwrap();
	let (area, epoch, generation, revision): (Uuid, i64, i64, i64) = sqlx::query_as("SELECT a.id, a.epoch, a.generation, a.revision FROM core_areas a JOIN core_runs r ON r.area_id = a.id WHERE r.run_id = $1").bind(run.id).fetch_one(db).await.unwrap();
	sqlx::query("INSERT INTO core_operations (id, area_id, run_id, credential_id, tenant, principal, request_key, digest, kind, state, epoch, generation, revision, policy_revision, subjects, input, result) VALUES ($1,$2,$3,$4,'acme','alice','wait-fixture','fixture','shell','running',$5,$6,$7,2,'[]'::jsonb,'{\"seconds\":3600}'::jsonb,'{}'::jsonb)").bind(operation).bind(area).bind(run.id).bind(credential).bind(epoch).bind(generation).bind(revision).execute(db).await.unwrap();
	let harness = aidash_server::harness::Harness {
		federation: f.clone(),
	};
	for _ in 0..20 {
		let state = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
		if matches!(state.state, aidash_server::domain::RunState::Waiting(ref w) if matches!(w.as_ref(), aidash_server::domain::WaitingState::Operation { .. }))
		{
			break;
		}
		assert!(
			harness.worker_once().await.unwrap(),
			"worker did not advance: {state:?}"
		);
	}
	let state = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	assert!(
		matches!(state.state, aidash_server::domain::RunState::Waiting(ref w) if matches!(w.as_ref(), aidash_server::domain::WaitingState::Operation { .. })),
		"{state:?}"
	);
	assert!(
		!harness.worker_once().await.unwrap(),
		"future wait should not occupy a worker"
	);
	assert_eq!(calls.load(Ordering::SeqCst), 1);
	sqlx::query("UPDATE core_operations SET state = 'completed' WHERE id = $1")
		.bind(operation)
		.execute(db)
		.await
		.unwrap();
	for _ in 0..20 {
		let state = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
		if state.phase().as_str() == "COMPLETED" {
			break;
		}
		assert!(
			harness.worker_once().await.unwrap(),
			"completion did not wake Run: {state:?}"
		);
	}
	let state = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	assert_eq!(state.phase().as_str(), "COMPLETED", "{state:?}");
	assert_eq!(calls.load(Ordering::SeqCst), 2);
	drop(app);
	drop(provider);
	cleanup(f, &url, &schema).await;
}
