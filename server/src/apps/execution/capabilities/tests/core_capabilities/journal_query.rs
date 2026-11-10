//! Builtin-to-Runner/1 integration with a recording protocol peer.
use super::*;
use aidash_application::ports::execution::ExecutionStore as _;
use aidash_domain::context::ContextEvent;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::sync::Mutex;

#[rstest::rstest]
#[tokio::test]
async fn builtin_sends_the_current_journal_to_the_same_run_session(
	#[future] capability_fixture: CoreFixture,
) {
	let mut c = capability_fixture.await;
	let (status, body) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/tasks/{}/delegate", c.task),
		json!({"node_id":c.f.config.deployment,"agent":{"id":"research","version":"1.1.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let worker = Uuid::new_v4();
	let mut run = c.f.store.lease_run(worker, 300).await.unwrap().unwrap();
	let executor = c.f.store.for_run(TEST_FIXTURE);
	let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
	let records = calls.clone();
	let profile = (*c.f.store.capabilities.0).clone();
	let health = json!({"protocol":"aidash-runner/1","verified":true,"python_verified":true,
		"image":"fixture@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
		"runtime_class":"fixture","instance":"runner-one","probe":{"resources":{
		"cpu":profile.cpu,"memory_bytes":profile.memory_bytes,"swap_bytes":0,"processes":profile.processes,
		"working_bytes":profile.working_bytes,"temporary_bytes":profile.temporary_bytes}}});
	let router = reinhardt::ServerRouter::new().handler("/{*rest}", upstream_fixtures::any_handler(move |request: reinhardt::Request| {
		assert_eq!(request.headers.get("authorization").unwrap(), "Bearer aidash-command-cleanup-fixture");
		let path = request.uri.path().to_owned();
		let body = request.json::<Value>().unwrap_or(Value::Null);
		let mut calls = records.lock().unwrap();
		calls.push(json!({"path":path,"body":body}));
		let response = if path == "/v1/health" { health.clone() }
		else if path == "/v1/operations" { json!({"status":"awaiting_files"}) }
		else if path.contains("/inputs/") { json!({"received":true}) }
		else if path.ends_with("/start") {
			let uploaded = calls.iter().rev().find(|call| call["path"].as_str().is_some_and(|path| path.contains("/inputs/"))).unwrap();
			let bytes = STANDARD.decode(uploaded["body"]["data"].as_str().unwrap()).unwrap();
			let latest = std::str::from_utf8(&bytes).unwrap().lines().last().unwrap_or("");
			json!({"status":"completed","writer_frozen":true,"stdout":STANDARD.encode(latest),"truncated":false})
		} else if path.starts_with("/v1/sessions/") && !path.ends_with("/stop") { json!({"live":true}) }
		else if path.ends_with("/stop") { json!({"termination_confirmed":true}) }
		else if path.ends_with("/ack") { json!({"acknowledged":true}) }
		else { panic!("unexpected runner request: {path}") };
		async move { reinhardt::Response::ok().with_json(&response).unwrap() }
	}));
	let runner = upstream_fixtures::upstream(Arc::new(router)).await;
	let mut profile = profile;
	profile.admission = true;
	profile.runner = Some(
		serde_json::from_value(
			json!({"endpoint":runner.url,"token_env":"AIDASH_COMMAND_CLEANUP_TEST_TOKEN",
		"image":"fixture@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
		"runtime_class":"fixture","namespace":"fixture"}),
		)
		.unwrap(),
	);
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	let tool = aidash_server::tool::builtins()
		.remove("journal_query")
		.unwrap();
	for (index, text) in ["first journal entry", "second journal entry"]
		.into_iter()
		.enumerate()
	{
		run.context.push(ContextEvent::ModelMediaObservation {
			text: text.into(),
			through_seq: None,
			truncated: false,
		});
		executor.journal_context(&run, worker).await.unwrap();
		let key = format!("journal-cell-{index}");
		let input = json!({"code":"journal()"});
		c.f.store
			.invocation_start(
				&TEST_FIXTURE,
				&run,
				worker,
				&key,
				"journal_query",
				&input,
				true,
			)
			.await
			.unwrap();
		let context = aidash_server::tool::ToolContext {
			home: aidash_server::federation::Home::new(c.f.clone(), run.clone()),
			store: c.f.store.clone(),
			run: run.clone(),
			scope: TEST_FIXTURE,
		};
		let output = tool.invoke(&context, input, &key).await.unwrap();
		assert!(
			output["stdout"].as_str().unwrap().contains(text),
			"{output}"
		);
		c.f.store
			.invocation_finish(&TEST_FIXTURE, &run, worker, &key, &output)
			.await
			.unwrap();
	}
	// A withdrawal in the exported Journal ends the old heap before another
	// cell can run, then a subsequent invocation receives a fresh session.
	run.context.push(ContextEvent::tool(
		aidash_domain::provider::ToolCall {
			id: "call-withdrawn".into(),
			name: "research_fetch".into(),
			arguments: json!({}),
		},
		ContextEvent::revoked_result("call-withdrawn"),
	));
	executor.journal_context(&run, worker).await.unwrap();
	let context = aidash_server::tool::ToolContext {
		home: aidash_server::federation::Home::new(c.f.clone(), run.clone()),
		store: c.f.store.clone(),
		run: run.clone(),
		scope: TEST_FIXTURE,
	};
	let input = json!({"code":"journal()"});
	c.f.store
		.invocation_start(
			&TEST_FIXTURE,
			&run,
			worker,
			"journal-reset",
			"journal_query",
			&input,
			true,
		)
		.await
		.unwrap();
	let reset = tool
		.invoke(&context, input.clone(), "journal-reset")
		.await
		.unwrap();
	assert_eq!(reset["error"]["code"], "SESSION_RESET");
	c.f.store
		.invocation_finish(&TEST_FIXTURE, &run, worker, "journal-reset", &reset)
		.await
		.unwrap();
	c.f.store
		.invocation_start(
			&TEST_FIXTURE,
			&run,
			worker,
			"journal-fresh",
			"journal_query",
			&input,
			true,
		)
		.await
		.unwrap();
	let fresh = tool.invoke(&context, input, "journal-fresh").await.unwrap();
	assert!(fresh["stdout"].as_str().unwrap().contains("call-withdrawn"));
	c.f.store
		.invocation_finish(&TEST_FIXTURE, &run, worker, "journal-fresh", &fresh)
		.await
		.unwrap();
	let requests = calls.lock().unwrap();
	let sessions: Vec<_> = requests
		.iter()
		.filter(|call| call["path"] == "/v1/operations")
		.map(|call| call["body"]["session_id"].clone())
		.collect();
	for request in requests
		.iter()
		.filter(|call| call["path"] == "/v1/operations")
	{
		assert_eq!(request["body"]["kind"], "journal");
		assert_eq!(request["body"]["runner_instance"], "runner-one");
		assert_eq!(request["body"]["files"][0]["scope"], "journal");
		assert!(request["body"].get("egress").is_none());
	}
	assert_eq!(sessions.len(), 3);
	assert_eq!(sessions[0], sessions[1]);
	assert_ne!(sessions[1], sessions[2]);
	assert!(requests.iter().any(|call| {
		call["path"]
			.as_str()
			.is_some_and(|path| path.ends_with("/stop"))
	}));
	assert!(
		requests
			.iter()
			.filter(|call| call["path"]
				.as_str()
				.is_some_and(|path| path.ends_with("/stop")))
			.all(|call| call["body"]["instance"] == "runner-one")
	);
	assert_eq!(
		requests
			.iter()
			.filter(|call| call["path"]
				.as_str()
				.is_some_and(|path| path.ends_with("/ack")))
			.count(),
		3
	);
	drop(requests);
	use reinhardt::query::{
		Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	};
	sqlx::query(
		&Query::update()
			.table(Alias::new("runs"))
			.value(Alias::new("phase"), "COMPLETED")
			.and_where(Expr::col("id").eq(Expr::value(run.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	let (stop, stopping) = tokio::sync::watch::channel(false);
	let cleanup = tokio::spawn(aidash_server::capabilities::reclamation::run(
		c.f.store.clone(),
		stopping,
	));
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
	loop {
		let count: i64 = sqlx::query_scalar(
			&Query::select()
				.expr(reinhardt::query::Func::count(Expr::asterisk().into()))
				.from(Alias::new("core_records"))
				.and_where(Expr::col("kind").eq("journal_session"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
		if count == 0 {
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"terminal Run's kernel was not reclaimed"
		);
		tokio::time::sleep(std::time::Duration::from_millis(50)).await;
	}
	stop.send(true).unwrap();
	cleanup.await.unwrap().unwrap();
	assert!(
		calls
			.lock()
			.unwrap()
			.iter()
			.filter(|call| call["path"]
				.as_str()
				.is_some_and(|path| path.ends_with("/stop")))
			.count() >= 2
	);
	c.close().await;
	drop(runner);
}
