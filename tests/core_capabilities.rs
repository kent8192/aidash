mod common;
use common::{TestEnvironment, bootstrap, cleanup, request, setup, test_environment};
use serde_json::json;
use std::sync::Arc;

#[rstest::rstest]
#[tokio::test]
async fn working_files_require_explicit_agent_settings(
	#[future] test_environment: Arc<TestEnvironment>,
) {
	let environment = test_environment.await;
	let (f, url, schema) = setup(&environment).await;
	let app = aidash::api::router(f.clone());
	let (_policy, token, task) = bootstrap(&f, &app, "http://localhost:19999").await;
	let (status, delegation) = request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{task}/delegate"),
		json!({"node_id":f.config.node_id,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{delegation}");
	let (status, result) = request(&app, &token, "GET", "/api/working-areas", json!(null)).await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(
		result["items"],
		json!([]),
		"legacy Agents must not gain working files"
	);
	cleanup(f, &url, &schema).await;
}

use aidash::{
	capabilities::{Profile, Runtime},
	federation::Federation,
};
use axum::Router;
use serde_json::Value;
use uuid::Uuid;

struct CoreFixture {
	_environment: Arc<TestEnvironment>,
	f: Federation,
	app: Router,
	token: String,
	policy: Value,
	task: Uuid,
	url: String,
	schema: String,
	root: std::path::PathBuf,
}
impl CoreFixture {
	async fn close(self) {
		if let Some(runner) = &self.f.store.capabilities.0.runner {
			use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
			// Tear down only this fixture's persisted interpreter identities. The
			// real node guardian proves termination before the database is removed.
			let sessions: Vec<Value> = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("data"))
					.from(Alias::new("core_records"))
					.and_where(Expr::col(Alias::new("kind")).eq("python_session"))
					.and_where(
						Expr::col(Alias::new("state")).is_in(["initial", "frozen", "running"]),
					)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&self.f.store.pool)
			.await
			.unwrap();
			let client = reqwest::Client::builder()
				.no_proxy()
				.timeout(std::time::Duration::from_secs(60))
				.build()
				.unwrap();
			for session in sessions {
				let sid: Uuid = serde_json::from_value(session["session_id"].clone()).unwrap();
				let reply: Value = client
					.post(format!(
						"{}/v1/sessions/{sid}/stop",
						runner.endpoint.trim_end_matches('/')
					))
					.bearer_auth(std::env::var(&runner.token_env).unwrap())
					.send()
					.await
					.unwrap()
					.error_for_status()
					.unwrap()
					.json()
					.await
					.unwrap();
				assert_eq!(
					reply["termination_confirmed"], true,
					"fixture interpreter must be stopped: {reply}"
				);
			}
		}
		cleanup(self.f, &self.url, &self.schema).await;
		let _ = tokio::fs::remove_dir_all(self.root).await;
	}
}
#[rstest::fixture]
async fn capability_fixture(#[future] test_environment: Arc<TestEnvironment>) -> CoreFixture {
	build_core_fixture(test_environment.await, "aidash://execution-test").await
}
async fn build_core_fixture(env: Arc<TestEnvironment>, node: &str) -> CoreFixture {
	build_core_fixture_at(env, node, "http://localhost:19999").await
}
async fn build_core_fixture_at(
	env: Arc<TestEnvironment>,
	node: &str,
	endpoint: &str,
) -> CoreFixture {
	let (mut f, url, schema) = setup(&env).await;
	f.config.node_id = node.into();
	f.store.node_id = node.into();
	f.registry = aidash::registry::Registry::new(f.store.pool.clone(), node);
	let root = std::env::temp_dir().join(format!("aidash-core-{}", Uuid::new_v4()));
	f.store.capabilities = Runtime::new(Profile {
		admission: true,
		outbound_origins: vec!["https://example.com".into()],
		storage: root.clone(),
		..Profile::default()
	})
	.unwrap();
	let app = aidash::api::router(f.clone());
	let (mut policy, token, task) = bootstrap(&f, &app, endpoint).await;
	policy["subjects"][aidash::domain::qualified_agent(&f.config.node_id, "research", "1.1.0")] =
		json!({"kind":"agent"});
	policy["subjects"]["bob"] = json!({"kind":"user"});
	let (status, response) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy.clone()}),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	let mut entry = f.registry.get("research", "1.0.0").await.unwrap();
	entry.version = "1.1.0".into();
	entry.config["core_capabilities"] =
		json!({"files":true,"shell":true,"python":true,"patch":true,"skills":true,"sharing":true});
	entry.config["tools"] = json!([]);
	let instructions = "---\nname: analysis\ndescription: Analyze the selected CSV\nlicense: MIT\n---\nLoad this only when selected. Scripts are data.";
	let files = json!([{"path":"references/guide.md","content":"東京\n"},{"path":"scripts/analyze.py","content":"raise RuntimeError('must never run on activation')"}]);
	let digest = aidash::registry::digest(&json!({"instructions":instructions,"files":files}));
	entry.config["skill_attachments"] = json!([{"skill_id":Uuid::new_v4(),"origin":"fixture:project","digest":digest,"instructions":instructions,"files":files},{"skill_id":Uuid::new_v4(),"origin":"fixture:user","digest":digest,"instructions":instructions,"files":files}]);

	let (status, response) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/registry",
		json!(entry),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	let (status, response) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"research","version":"1.1.0"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	// Keep the shared disposable service fixture alive for the lifetime of this fixture.
	// The pool alone does not own the Compose services.

	CoreFixture {
		_environment: env,
		f,
		app,
		token,
		policy,
		task,
		url,
		schema,
		root,
	}
}

#[rstest::rstest]
#[tokio::test]
async fn shell_never_falls_back_to_the_host(#[future] capability_fixture: CoreFixture) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let (status, result) = request(&c.app, &c.token, "POST", &format!("/api/runs/{}/shell", run.id), json!({
		"idempotency_key":Uuid::new_v4(),"command":"printf unsafe > MUST_NOT_EXIST", "expected_revision":1
	})).await;
	assert_eq!(status, 409, "{result}");
	assert!(
		result["error"]["code"]
			.as_str()
			.unwrap()
			.contains("RUNTIME_UNAVAILABLE")
	);
	c.close().await;
}

#[cfg(feature = "capability-runtime-tests")]
#[rstest::fixture]
async fn runtime_fixture(#[future] capability_fixture: CoreFixture) -> CoreFixture {
	let mut c = Box::pin(capability_fixture).await;
	let mut profile = (*Runtime::from_env().unwrap().0).clone();
	assert!(
		profile.runner.is_some(),
		"set AIDASH_CAPABILITY_PROFILE to a verified isolated runner profile"
	);
	profile.storage = c.root.clone();
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	c.app = aidash::api::router(c.f.clone());
	c
}

#[cfg(feature = "capability-runtime-tests")]
async fn operation_until(
	c: &CoreFixture,
	run: Uuid,
	kind: &str,
	operation: &Value,
	wanted: &[&str],
) -> Value {
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
	loop {
		let (status, result) = request(
			&c.app,
			&c.token,
			"POST",
			&format!("/api/runs/{run}/{kind}/poll"),
			json!({"operation_id":operation}),
		)
		.await;
		assert_eq!(status, 200, "{result}");
		if wanted.contains(&result["status"].as_str().unwrap()) {
			return result;
		}
		assert!(
			!matches!(
				result["status"].as_str(),
				Some("failed" | "uncertain" | "withdrawn")
			),
			"unexpected terminal result: {result}"
		);
		assert!(
			tokio::time::Instant::now() < deadline,
			"operation did not reach {wanted:?}: {result}"
		);
		tokio::time::sleep(std::time::Duration::from_millis(200)).await;
	}
}

#[cfg(feature = "capability-runtime-tests")]
#[rstest::rstest]
#[tokio::test]
async fn isolated_shell_survives_worker_restart_and_confirms_descendant_cancellation(
	#[future] runtime_fixture: CoreFixture,
) {
	let c = Box::pin(runtime_fixture).await;
	let run = admit(&c).await;
	let (stopping, receiver) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(
		c.f.store.clone(),
		receiver.clone(),
	));
	let path = format!("/api/runs/{}/shell", run.id);
	let input = json!({"idempotency_key":Uuid::new_v4(),"command":"printf '一度だけ\\n' >> count.txt; sleep 5; printf 'finished\\n'", "expected_revision":1});
	let (status, first) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{first}");
	assert_eq!(first["status"], "prepared");
	let (status, busy) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/files/search", run.id),
		json!({"query":"x","mode":"literal","scope":"working"}),
	)
	.await;
	assert_eq!(status, 409, "{busy}");
	operation_until(&c, run.id, "shell", &first["operation_id"], &["running"]).await;
	worker.abort();
	let _ = worker.await;
	let worker = tokio::spawn(aidash::capabilities::operations::run(
		c.f.store.clone(),
		receiver,
	));
	let (status, repeated) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{repeated}");
	assert_eq!(repeated["operation_id"], first["operation_id"]);
	let mut changed = input;
	changed["command"] = json!("printf duplicate >> count.txt");
	assert_eq!(
		request(&c.app, &c.token, "POST", &path, changed).await.0,
		409
	);
	let finished =
		operation_until(&c, run.id, "shell", &first["operation_id"], &["completed"]).await;
	assert_eq!(finished["termination_confirmed"], true);
	assert!(finished["output"].as_str().unwrap().contains("finished"));
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	let file = area["manifest"]
		.as_array()
		.unwrap()
		.iter()
		.find(|file| file["path"] == "count.txt")
		.unwrap();
	let (status, read) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/files/read", run.id),
		json!({"file_id":file["file_id"],"representation":"text"}),
	)
	.await;
	assert_eq!(status, 200, "{read}");
	assert_eq!(read["content"], "一度だけ\n");
	let (status, long) = request(&c.app,&c.token,"POST",&path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"command":"python -c 'import os,signal,time; os.setsid(); signal.signal(signal.SIGTERM,signal.SIG_IGN); time.sleep(60)' & wait"})).await;
	assert_eq!(status, 200, "{long}");
	operation_until(&c, run.id, "shell", &long["operation_id"], &["running"]).await;
	let (status, cancel) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/shell/cancel", run.id),
		json!({"operation_id":long["operation_id"]}),
	)
	.await;
	assert_eq!(status, 200, "{cancel}");
	let cancelled =
		operation_until(&c, run.id, "shell", &long["operation_id"], &["cancelled"]).await;
	assert_eq!(cancelled["termination_confirmed"], true);
	assert_eq!(cancelled["effects_may_have_occurred"], true);
	stopping.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}
#[cfg(feature = "capability-runtime-tests")]
#[rstest::rstest]
#[tokio::test]
async fn isolated_python_preserves_heap_and_requires_reset_acknowledgement(
	#[future] runtime_fixture: CoreFixture,
) {
	let c = Box::pin(runtime_fixture).await;
	let run = admit(&c).await;
	let (stop, receiver) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(
		c.f.store.clone(),
		receiver,
	));
	let path = format!("/api/runs/{}/python", run.id);
	let input = json!({"idempotency_key":Uuid::new_v4(),"code":"counter = 41\nfrom pathlib import Path\nPath('value.txt').write_text('東京')\nprint(counter)","expected_revision":1});
	let (status, first) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{first}");
	let done = operation_until(&c, run.id, "python", &first["operation_id"], &["completed"]).await;
	assert_eq!(done["writer_frozen"], true, "{done}");
	assert!(done["output"].as_str().unwrap().contains("41"));
	let (status, duplicate) = request(&c.app, &c.token, "POST", &path, input).await;
	assert_eq!(status, 200, "{duplicate}");
	assert_eq!(duplicate["operation_id"], first["operation_id"]);
	let input = json!({"idempotency_key":Uuid::new_v4(),"code":"counter += 1\nprint(counter)","expected_revision":done["revision"],"expected_session_id":done["session_id"]});
	let (status, next) = request(&c.app, &c.token, "POST", &path, input).await;
	assert_eq!(status, 200, "{next}");
	let second = operation_until(&c, run.id, "python", &next["operation_id"], &["completed"]).await;
	assert_eq!(second["session_id"], done["session_id"]);
	assert!(second["output"].as_str().unwrap().contains("42"));
	let (status,patched)=request(&c.app,&c.token,"POST",&format!("/api/runs/{}/patch",run.id),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":second["revision"],"patch":"*** Begin Patch\n*** Add File: changed.txt\n+changed outside Python\n*** End Patch","preconditions":{"changed.txt":null}})).await;
	assert_eq!(status, 200, "{patched}");
	let mut input = json!({"idempotency_key":Uuid::new_v4(),"code":"print(counter)","expected_revision":patched["revision"],"expected_session_id":second["session_id"]});
	let (status, reset) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{reset}");
	assert_eq!(reset["error"]["code"], "SESSION_RESET");
	assert_ne!(reset["session_id"], second["session_id"]);
	input["expected_session_id"] = reset["session_id"].clone();
	let (status, ack) = request(&c.app, &c.token, "POST", &path, input).await;
	assert_eq!(status, 200, "{ack}");
	let failed = operation_until(&c, run.id, "python", &ack["operation_id"], &["failed"]).await;
	assert!(
		failed["error"]["details"]["type"] == "NameError",
		"{failed}"
	);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}
async fn admit(c: &CoreFixture) -> aidash::domain::Run {
	let (status, response) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/tasks/{}/delegate", c.task),
		json!({"node_id":c.f.config.node_id,"agent":{"id":"research","version":"1.1.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	c.f.store.runs().await.unwrap().remove(0)
}
async fn source(c: &CoreFixture, workspace: Uuid, text: &str) -> Uuid {
	let (status, response) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/workspaces/{workspace}/thread-messages"),
		json!({"content":text,"idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	serde_json::from_value(response["message"]["id"].clone()).unwrap()
}
#[rstest::rstest]
#[tokio::test]
async fn files_preserve_unicode_revision_and_source_authority(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let message = source(&c, run.workspace_id, &"東京の資料\n".repeat(61)).await;
	let input = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"path":"notes/日本語.txt","source":{"kind":"message","message_id":message}});
	let path = format!("/api/runs/{}/files/materialize", run.id);
	let (status, created) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{created}");
	assert_eq!(
		request(&c.app, &c.token, "POST", &path, input.clone()).await,
		(200, created.clone())
	);
	let mut changed = input;
	changed["path"] = json!("other.txt");
	assert_eq!(
		request(&c.app, &c.token, "POST", &path, changed).await.0,
		409
	);
	let read_path = format!("/api/runs/{}/files/read", run.id);
	let file = created["file"]["file_id"].clone();
	let (status, page) = request(
		&c.app,
		&c.token,
		"POST",
		&read_path,
		json!({"file_id":file,"representation":"text","max_bytes":4}),
	)
	.await;
	assert_eq!(status, 200, "{page}");
	assert_eq!(page["content"], "東");
	assert_eq!(page["next_offset"], 3);
	assert_eq!(
		request(
			&c.app,
			&c.token,
			"POST",
			&read_path,
			json!({"file_id":file,"representation":"text","offset":1})
		)
		.await
		.0,
		400
	);
	let search_path = format!("/api/runs/{}/files/search", run.id);
	let query = json!({"query":"東京","mode":"literal","scope":"working"});
	let (status, first) = request(&c.app, &c.token, "POST", &search_path, query.clone()).await;
	assert_eq!(status, 200, "{first}");
	assert_eq!(first["matches"].as_array().unwrap().len(), 50);
	let mut next = query;
	next["cursor"] = first["next_cursor"].clone();
	let (status, page) = request(&c.app, &c.token, "POST", &search_path, next.clone()).await;
	assert_eq!(status, 200, "{page}");
	assert_eq!(page["matches"].as_array().unwrap().len(), 11);
	let (_,more)=request(&c.app,&c.token,"POST",&path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":2,"path":"extra.txt","source":{"kind":"message","message_id":message}})).await;
	assert_eq!(more["revision"], 3);
	assert_eq!(
		request(&c.app, &c.token, "POST", &search_path, next)
			.await
			.0,
		409
	);
	let (_, bob) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	let (status, result) = request(
		&c.app,
		bob["token"].as_str().unwrap(),
		"POST",
		&read_path,
		json!({"file_id":file,"representation":"metadata"}),
	)
	.await;
	assert_eq!(status, 404, "{result}");
	assert_eq!(
		request(
			&c.app,
			&c.token,
			"POST",
			&read_path,
			json!({"file_id":Uuid::new_v4(),"representation":"metadata"})
		)
		.await
		.0,
		404
	);
	let mut revoked = c.policy.clone();
	revoked["policies"].as_array_mut().unwrap().push(json!({
		"id":"revoke-source", "effect":"deny", "subjects":{"any":true},
		"actions":["message.read"], "resources":{"kinds":["message"],"ids":[message.to_string()]}
	}));
	let (status, result) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":revoked}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	let (status, result) = request(
		&c.app,
		&c.token,
		"POST",
		&read_path,
		json!({"file_id":file,"representation":"text"}),
	)
	.await;
	assert!(
		matches!(status, 403 | 404),
		"revoked source must withdraw derived text: {status} {result}"
	);
	c.close().await;
}
#[rstest::rstest]
#[tokio::test]
async fn queued_runs_keep_order_and_cancellation_bypasses_queue(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
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
	let queue_path = format!("/api/working-areas/{}/queue", area["id"].as_str().unwrap());
	let input = json!({"idempotency_key":Uuid::new_v4(),"title":"Follow up","description":"Use saved files","agent_version":"1.1.0"});
	let (status, queue) = request(&c.app, &c.token, "POST", &queue_path, input.clone()).await;
	assert_eq!(status, 200, "{queue}");
	assert_eq!(queue["queue"].as_array().unwrap().len(), 2);
	let steer_path = format!("/api/working-areas/{}/steer", area["id"].as_str().unwrap());
	let (status, result) = request(&c.app, &c.token, "POST", &steer_path, json!({"idempotency_key":Uuid::new_v4(),"expected_run_id":queue["queue"][1]["run_id"],"content":"Must not reach a queued run"})).await;
	assert_eq!(status, 409, "{result}");
	let steer = json!({"idempotency_key":Uuid::new_v4(),"expected_run_id":run.id,"content":"Prioritize the current question"});
	let (_, bob) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	assert_eq!(
		request(
			&c.app,
			bob["token"].as_str().unwrap(),
			"POST",
			&steer_path,
			steer.clone()
		)
		.await
		.0,
		404
	);
	assert!(
		c.f.store.run_inputs(run.id).await.unwrap().is_empty(),
		"another requester cannot steer a privileged Run"
	);
	let (status, result) = request(&c.app, &c.token, "POST", &steer_path, steer.clone()).await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(
		request(&c.app, &c.token, "POST", &steer_path, steer)
			.await
			.0,
		200
	);
	let inputs = c.f.store.run_inputs(run.id).await.unwrap();
	assert_eq!(inputs.len(), 1);
	assert_eq!(inputs[0].content, "Prioritize the current question");
	assert_eq!(
		request(&c.app, &c.token, "POST", &queue_path, input.clone()).await,
		(200, queue.clone())
	);
	let mut conflict = input;
	conflict["description"] = json!("different work");
	assert_eq!(
		request(&c.app, &c.token, "POST", &queue_path, conflict)
			.await
			.0,
		409
	);
	let worker = Uuid::new_v4();
	let leased = c.f.store.lease_run(worker, 30).await.unwrap().unwrap();
	assert_eq!(leased.id, run.id);
	assert!(
		c.f.store
			.lease_run(Uuid::new_v4(), 30)
			.await
			.unwrap()
			.is_none(),
		"another worker cannot overtake active work"
	);
	let queued: Uuid = serde_json::from_value(queue["queue"][1]["run_id"].clone()).unwrap();
	let (status, result) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{queued}/control"),
		json!({"action":"cancel"}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(
		c.f.store
			.lease_run(Uuid::new_v4(), 30)
			.await
			.unwrap()
			.unwrap()
			.id,
		queued,
		"cancellation must not wait behind the active Run"
	);
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn patches_publish_all_files_or_keep_the_previous_revision(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let path = format!("/api/runs/{}/patch", run.id);
	let input = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,
        "preconditions":{"a.txt":null,"b.txt":null},
        "patch":"*** Begin Patch\n*** Add File: a.txt\n+東京\n*** Add File: b.txt\n+remove me\n*** End Patch"});
	let (status, added) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{added}");
	assert_eq!(added["revision"], 2);
	assert_eq!(
		request(&c.app, &c.token, "POST", &path, input).await,
		(200, added.clone())
	);
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	let entries = area["manifest"].as_array().unwrap();
	let a = entries.iter().find(|f| f["path"] == "a.txt").unwrap();
	let b = entries.iter().find(|f| f["path"] == "b.txt").unwrap();
	let change = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":2,
        "preconditions":{"a.txt":a["digest"],"b.txt":b["digest"]},
        "patch":"*** Begin Patch\n*** Update File: a.txt\n@@\n-東京\n+京都\n*** Delete File: b.txt\n*** End Patch"});
	let mut invalid = change.clone();
	invalid["patch"] = json!(
		"*** Begin Patch\n*** Delete File: b.txt\n*** Update File: a.txt\n@@\n-missing context\n+bad\n*** End Patch"
	);
	let (status, rejected) = request(&c.app, &c.token, "POST", &path, invalid).await;
	assert_eq!(status, 409, "{rejected}");
	let (_, unchanged) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(unchanged["revision"], 2);
	assert_eq!(unchanged["manifest"], area["manifest"]);
	let (status, updated) = request(&c.app, &c.token, "POST", &path, change).await;
	assert_eq!(status, 200, "{updated}");
	assert_eq!(updated["revision"], 3);
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(area["manifest"].as_array().unwrap().len(), 1);
	let (status, content) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/files/read", run.id),
		json!({"file_id":area["manifest"][0]["file_id"],"representation":"text"}),
	)
	.await;
	assert_eq!(status, 200, "{content}");
	assert_eq!(content["content"], "京都\n");
	for (revision, path, expected) in [
		(2, "a.txt", a["digest"].clone()),
		(3, "../outside", Value::Null),
		(3, "a.txt", Value::Null),
	] {
		let (status,_)=request(&c.app,&c.token,"POST",&path_for_patch(run.id),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":revision,"preconditions":{path:expected},"patch":format!("*** Begin Patch\n*** Add File: {path}\n+bad\n*** End Patch")})).await;
		assert!(matches!(status, 400 | 409));
	}
	c.close().await;
}
fn path_for_patch(id: Uuid) -> String {
	format!("/api/runs/{id}/patch")
}

#[rstest::rstest]
#[tokio::test]
async fn direct_skills_are_pinned_and_loaded_progressively(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let base = format!("/api/runs/{}/skills", run.id);
	let (status, list) =
		request(&c.app, &c.token, "POST", &format!("{base}/list"), json!({})).await;
	assert_eq!(status, 200, "{list}");
	let skills = list["skills"].as_array().unwrap();
	assert_eq!(skills.len(), 2);
	assert_eq!(skills[0]["name"], skills[1]["name"]);
	assert_ne!(skills[0]["skill_id"], skills[1]["skill_id"]);
	assert_ne!(skills[0]["origin"], skills[1]["origin"]);
	assert!(!list.to_string().contains("Load this only"));
	let skill = &skills[0];
	let (status, loaded) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("{base}/load"),
		json!({"skill_id":skill["skill_id"],"expected_digest":skill["digest"]}),
	)
	.await;
	assert_eq!(status, 200, "{loaded}");
	assert!(
		loaded["content"]
			.as_str()
			.unwrap()
			.contains("Load this only")
	);
	assert_eq!(loaded["skill"]["license"], "MIT");
	let (status,read)=request(&c.app,&c.token,"POST",&format!("{base}/read"),json!({"skill_id":skill["skill_id"],"digest":skill["digest"],"path":"references/guide.md","max_bytes":4})).await;
	assert_eq!(status, 200, "{read}");
	assert_eq!(read["content"], "東");
	assert_eq!(read["next_offset"], 3);
	for (path, digest, expected) in [
		("references/guide.md", "stale", 409),
		(
			"../scripts/analyze.py",
			skill["digest"].as_str().unwrap(),
			400,
		),
	] {
		let (status, result) = request(
			&c.app,
			&c.token,
			"POST",
			&format!("{base}/read"),
			json!({"skill_id":skill["skill_id"],"digest":digest,"path":path}),
		)
		.await;
		assert_eq!(status, expected, "{result}");
	}
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(area["state"], "active");
	assert_eq!(area["manifest"], json!([]));
	assert_eq!(area["epoch"], 1);
	c.close().await;
}

#[rstest::fixture]
async fn approval_fixture(
	#[future] capability_fixture: CoreFixture,
) -> (CoreFixture, aidash::domain::Run) {
	configure_approvals(Box::pin(capability_fixture).await).await
}
async fn configure_approvals(mut c: CoreFixture) -> (CoreFixture, aidash::domain::Run) {
	let run = admit(&c).await;
	c.policy["policies"][0]["resources"]["kinds"] = json!([
		"workspace",
		"task",
		"run",
		"tool",
		"message",
		"agent",
		"model",
		"skill",
		"memory",
		"working_area",
		"artifact",
		"human_request"
	]);
	c.policy["policies"].as_array_mut().unwrap().push(json!({"id":"approvable-outbound","effect":"allow","subjects":{"any":true},"actions":["capability.request","capability.approve"],"resources":{"kinds":["outbound"]}}));
	let (status, value) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":c.policy}),
	)
	.await;
	assert_eq!(status, 200, "{value}");
	(c, run)
}
#[rstest::rstest]
#[tokio::test]
async fn approvals_bind_invocations_and_never_override_upper_denials(
	#[future] approval_fixture: (CoreFixture, aidash::domain::Run),
) {
	let (c, run) = Box::pin(approval_fixture).await;
	let path = format!("/api/runs/{}/outbound", run.id);
	let input = json!({"idempotency_key":Uuid::new_v4(),"url":"https://example.com/a"});
	let (status, pending) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{pending}");
	assert_eq!(pending["status"], "approval_required");
	assert_eq!(pending["approver"], "alice");
	let decision_path = format!(
		"/api/capabilities/approvals/{}/decide",
		pending["approval_id"].as_str().unwrap()
	);
	let (_, bob) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	let decision = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1});
	assert_eq!(
		request(
			&c.app,
			bob["token"].as_str().unwrap(),
			"POST",
			&decision_path,
			decision.clone()
		)
		.await
		.0,
		404
	);
	let (status, allowed) =
		request(&c.app, &c.token, "POST", &decision_path, decision.clone()).await;
	assert_eq!(status, 200, "{allowed}");
	assert_eq!(allowed["state"], "approved");
	assert!(allowed["grant_id"].is_null());
	assert_eq!(
		request(&c.app, &c.token, "POST", &decision_path, decision).await,
		(200, allowed.clone())
	);
	assert_eq!(request(&c.app,&c.token,"POST",&decision_path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"choice":"allow_run","targets":["https://example.com"],"expires_at":chrono::Utc::now()+chrono::Duration::minutes(5)})).await.0,409);
	let (_, second) = request(
		&c.app,
		&c.token,
		"POST",
		&path,
		json!({"idempotency_key":Uuid::new_v4(),"url":"https://example.com/b"}),
	)
	.await;
	assert_eq!(second["status"], "approval_required");
	let (status, revoked) = request(
		&c.app,
		&c.token,
		"POST",
		&format!(
			"/api/capabilities/approvals/{}/revoke",
			pending["approval_id"].as_str().unwrap()
		),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":2}),
	)
	.await;
	assert_eq!(status, 200, "{revoked}");
	let (_, again) = request(&c.app, &c.token, "POST", &path, input).await;
	assert_eq!(again["status"], "blocked");
	let mut tightened = c.policy.clone();
	tightened["policies"].as_array_mut().unwrap().push(json!({"id":"upper-network-deny","effect":"deny","subjects":{"any":true},"actions":["network.get"],"resources":{"kinds":["outbound"]}}));
	let (status, value) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":3,"bundle":tightened}),
	)
	.await;
	assert_eq!(status, 200, "{value}");
	let (status, value) = request(
		&c.app,
		&c.token,
		"POST",
		&path,
		json!({"idempotency_key":Uuid::new_v4(),"url":"https://example.com/c"}),
	)
	.await;
	assert_eq!(status, 403, "{value}");
	c.close().await;
}
#[rstest::rstest]
#[tokio::test]
async fn reusable_grants_require_explicit_scope_and_revocation_withdraws_reuse(
	#[future] approval_fixture: (CoreFixture, aidash::domain::Run),
) {
	let (c, run) = Box::pin(approval_fixture).await;
	let path = format!("/api/runs/{}/outbound", run.id);
	let (_, pending) = request(
		&c.app,
		&c.token,
		"POST",
		&path,
		json!({"idempotency_key":Uuid::new_v4(),"url":"https://example.com/a"}),
	)
	.await;
	let decision_path = format!(
		"/api/capabilities/approvals/{}/decide",
		pending["approval_id"].as_str().unwrap()
	);
	assert_eq!(
		request(
			&c.app,
			&c.token,
			"POST",
			&decision_path,
			json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"choice":"allow_run"})
		)
		.await
		.0,
		400
	);
	let (status,grant)=request(&c.app,&c.token,"POST",&decision_path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"choice":"allow_run","targets":["https://example.com"],"expires_at":chrono::Utc::now()+chrono::Duration::minutes(5)})).await;
	assert_eq!(status, 200, "{grant}");
	let (_, reused) = request(
		&c.app,
		&c.token,
		"POST",
		&path,
		json!({"idempotency_key":Uuid::new_v4(),"url":"https://example.com/b"}),
	)
	.await;
	assert_eq!(reused["status"], "running");
	assert_eq!(reused["grant_id"], grant["grant_id"]);
	let (status, value) = request(
		&c.app,
		&c.token,
		"POST",
		&format!(
			"/api/capabilities/grants/{}/revoke",
			grant["grant_id"].as_str().unwrap()
		),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1}),
	)
	.await;
	assert_eq!(status, 200, "{value}");
	let (_, pending) = request(
		&c.app,
		&c.token,
		"POST",
		&path,
		json!({"idempotency_key":Uuid::new_v4(),"url":"https://example.com/c"}),
	)
	.await;
	assert_eq!(pending["status"], "approval_required");
	c.close().await;
}
#[rstest::rstest]
#[tokio::test]
async fn local_shares_are_fixed_recipient_owned_copies(#[future] capability_fixture: CoreFixture) {
	let c = capability_fixture.await;
	let sender = admit(&c).await;
	let mut reader = c.f.registry.get("research", "1.1.0").await.unwrap();
	reader.id = "reader".into();
	let (status, value) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/registry",
		json!(reader),
	)
	.await;
	assert_eq!(status, 200, "{value}");
	let (_, value) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"reader","version":"1.1.0"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(value["enabled"], true, "{value}");
	let mut policy = c.policy.clone();
	policy["subjects"][aidash::domain::qualified_agent(&c.f.config.node_id, "reader", "1.1.0")] =
		json!({"kind":"agent"});
	let (status, value) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{value}");
	let (status, task) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/workspaces/{}/tasks", sender.workspace_id),
		json!({"title":"Recipient","description":"Read a selected file"}),
	)
	.await;
	assert_eq!(status, 200, "{task}");
	let (status, value) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/tasks/{}/delegate", task["id"].as_str().unwrap()),
		json!({"node_id":c.f.config.node_id,"agent":{"id":"reader","version":"1.1.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{value}");
	let receiver =
		c.f.store
			.runs()
			.await
			.unwrap()
			.into_iter()
			.find(|r| r.agent_id == "reader")
			.unwrap();
	let patch_path = format!("/api/runs/{}/patch", sender.id);
	let (status,value)=request(&c.app,&c.token,"POST",&patch_path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"data.csv":null},"patch":"*** Begin Patch\n*** Add File: data.csv\n+value\n+first\n*** End Patch"})).await;
	assert_eq!(status, 200, "{value}");
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", sender.id),
		Value::Null,
	)
	.await;
	let source = &area["manifest"][0];
	let input = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":2,"files":[{"file_id":source["file_id"],"expected_digest":source["digest"]}],"recipient":{"node_id":c.f.config.node_id,"agent_id":"reader","agent_version":"1.1.0","thread_id":task["id"]}});
	let share_path = format!("/api/runs/{}/files/share", sender.id);
	let (status, receipt) = request(&c.app, &c.token, "POST", &share_path, input.clone()).await;
	assert_eq!(status, 200, "{receipt}");
	assert_eq!(receipt["status"], "completed");
	assert_eq!(
		request(&c.app, &c.token, "POST", &share_path, input.clone()).await,
		(200, receipt.clone())
	);
	let file = &receipt["receipt"]["files"][0];
	assert_ne!(file["file_id"], source["file_id"]);
	assert_eq!(file["digest"], source["digest"]);
	let (status,value)=request(&c.app,&c.token,"POST",&patch_path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":2,"preconditions":{"data.csv":source["digest"]},"patch":"*** Begin Patch\n*** Update File: data.csv\n@@\n-first\n+second\n*** End Patch"})).await;
	assert_eq!(status, 200, "{value}");
	let read_path = format!("/api/runs/{}/files/read", receiver.id);
	let (status, read) = request(
		&c.app,
		&c.token,
		"POST",
		&read_path,
		json!({"file_id":file["file_id"],"representation":"text"}),
	)
	.await;
	assert_eq!(status, 200, "{read}");
	assert_eq!(read["content"], "value\nfirst\n");
	let (status,copy)=request(&c.app,&c.token,"POST",&format!("/api/runs/{}/files/materialize",receiver.id),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":2,"path":"copy.csv","source":{"kind":"file","file_id":file["file_id"],"expected_digest":file["digest"]}})).await;
	assert_eq!(status, 200, "{copy}");
	assert_ne!(copy["file"]["file_id"], file["file_id"]);
	let mut conflict = input;
	conflict["files"][0]["expected_digest"] = json!("wrong");
	assert_eq!(
		request(&c.app, &c.token, "POST", &share_path, conflict)
			.await
			.0,
		409
	);
	c.close().await;
}

#[cfg(feature = "capability-runtime-tests")]
#[rstest::rstest]
#[tokio::test]
async fn reference_original_is_private_located_and_unchanged_by_python(
	#[future] runtime_fixture: CoreFixture,
) {
	use base64::Engine;
	let mut c = Box::pin(runtime_fixture).await;
	let bytes = include_bytes!("fixtures/core-references.xlsx");
	let digest = aidash::capabilities::objects::digest(bytes);
	let (stop, receiver) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(
		c.f.store.clone(),
		receiver,
	));
	let input = json!({"idempotency_key":Uuid::new_v4(),"name":"日本語.xlsx","media_type":"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet","size":bytes.len(),"digest":digest});
	let (status, upload) = request(
		&c.app,
		&c.token,
		"POST",
		"/api/references/uploads",
		input.clone(),
	)
	.await;
	assert_eq!(status, 200, "{upload}");
	assert_eq!(
		request(&c.app, &c.token, "POST", "/api/references/uploads", input)
			.await
			.1["reference_id"],
		upload["reference_id"]
	);
	let id = upload["reference_id"].as_str().unwrap();
	let path = format!("/api/references/{id}");
	let chunk = json!({"offset":0,"data":base64::engine::general_purpose::STANDARD.encode(bytes)});
	let (status, uploaded) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("{path}/chunks"),
		chunk.clone(),
	)
	.await;
	assert_eq!(status, 200, "{uploaded}");
	assert_eq!(
		request(&c.app, &c.token, "POST", &format!("{path}/chunks"), chunk)
			.await
			.1,
		uploaded
	);
	let (status, committed) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("{path}/commit"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{committed}");
	let until = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
	let ready = loop {
		let (status, value) = request(&c.app, &c.token, "GET", &path, Value::Null).await;
		assert_eq!(status, 200, "{value}");
		if value["state"] == "ready" {
			break value;
		}
		assert!(tokio::time::Instant::now() < until, "{value}");
		tokio::time::sleep(std::time::Duration::from_millis(200)).await;
	};
	assert_eq!(ready["extraction_state"], "ready", "{ready}");
	c.policy["subjects"]
		[aidash::domain::qualified_agent(&c.f.config.node_id, "research", "1.2.0")] =
		json!({"kind":"agent"});
	assert_eq!(
		request(
			&c.app,
			&c.f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":2,"bundle":c.policy})
		)
		.await
		.0,
		200
	);
	let mut agent = c.f.registry.get("research", "1.1.0").await.unwrap();
	agent.version = "1.2.0".into();
	agent.config["reference_attachments"] = json!([{"reference_id":id,"digest":digest}]);
	let (status, value) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/registry",
		json!(agent),
	)
	.await;
	assert_eq!(status, 200, "{value}");
	assert_eq!(
		request(
			&c.app,
			&c.f.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"research","version":"1.2.0"},"expected_revision":0,"enabled":true})
		)
		.await
		.0,
		200
	);
	let (status, value) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/tasks/{}/delegate", c.task),
		json!({"node_id":c.f.config.node_id,"agent":{"id":"research","version":"1.2.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{value}");
	let run = c.f.store.runs().await.unwrap().remove(0);
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	let (status, found) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/files/search", run.id),
		json!({"query":"東京","mode":"literal","scope":"references"}),
	)
	.await;
	assert_eq!(status, 200, "{found}");
	assert!(!found["matches"].as_array().unwrap().is_empty());
	let (status, located) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/files/read", run.id),
		json!({"file_id":ready["extraction"]["file_id"],"representation":"text"}),
	)
	.await;
	assert_eq!(status, 200, "{located}");
	assert!(
		located["content"]
			.as_str()
			.unwrap()
			.contains("Sheet Sales!A1")
	);
	let (status,copy)=request(&c.app,&c.token,"POST",&format!("/api/runs/{}/files/materialize",run.id),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"path":"copy.xlsx","source":{"kind":"file","file_id":ready["original"]["file_id"],"expected_digest":digest}})).await;
	assert_eq!(status, 200, "{copy}");
	assert_ne!(copy["file"]["file_id"], ready["original"]["file_id"]);
	let python_path = format!("/api/runs/{}/python", run.id);
	let (status,op)=request(&c.app,&c.token,"POST",&python_path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":copy["revision"],"code":"import openpyxl\nw=openpyxl.load_workbook('copy.xlsx')\nw['Sales']['B1']=99\nw.save('copy.xlsx')\nprint(w['Sales']['B1'].value)"})).await;
	assert_eq!(status, 200, "{op}");
	let done = operation_until(&c, run.id, "python", &op["operation_id"], &["completed"]).await;
	assert!(done["output"].as_str().unwrap().contains("99"));
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
		base64::engine::general_purpose::STANDARD
			.decode(original["data"].as_str().unwrap())
			.unwrap(),
		bytes
	);
	let (_, current) = request(&c.app, &c.token, "GET", &path, Value::Null).await;
	let (status, result) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("{path}/revoke"),
		json!({"expected_revision":current["revision"]}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(request(&c.app,&c.token,"POST",&python_path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":done["revision"],"expected_session_id":done["session_id"],"code":"print(w['Sales']['B1'].value)"})).await.0,404);
	assert_eq!(
		request(
			&c.app,
			&c.token,
			"GET",
			&format!("{path}/download"),
			Value::Null
		)
		.await
		.0,
		404
	);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn cleanup_restores_exact_bytes_and_fences_old_runs_and_confirmations(
	#[future] capability_fixture: CoreFixture,
) {
	let c = Box::pin(capability_fixture).await;
	let run = admit(&c).await;
	let (status,patch)=request(&c.app,&c.token,"POST",&path_for_patch(run.id),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"keep.txt":null},"patch":"*** Begin Patch\n*** Add File: keep.txt\n+保存する東京の資料\n*** End Patch"})).await;
	assert_eq!(status, 200, "{patch}");
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	let id = area["id"].as_str().unwrap();
	let path = format!("/api/working-areas/{id}/cleanup");
	let digest = area["manifest"][0]["digest"].clone();
	let (status, keep) = request(
		&c.app,
		&c.token,
		"POST",
		&path,
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"choice":"keep"}),
	)
	.await;
	assert_eq!(status, 200, "{keep}");
	assert_eq!(keep["state"], "kept");
	let (stop, receiver) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(
		c.f.store.clone(),
		receiver,
	));
	let input = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"choice":"recoverable"});
	let (status, cleanup) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{cleanup}");
	let status_path = format!(
		"/api/file-cleanups/{}",
		cleanup["operation_id"].as_str().unwrap()
	);
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
	let cleaned = loop {
		let (status, value) = request(&c.app, &c.token, "GET", &status_path, Value::Null).await;
		assert_eq!(status, 200, "{value}");
		if value["state"] == "recoverable" {
			break value;
		}
		assert!(tokio::time::Instant::now() < deadline, "{value}");
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	};
	assert_eq!(
		request(&c.app, &c.token, "POST", &path, input).await.1["operation_id"],
		cleanup["operation_id"]
	);
	assert!(
		request(
			&c.app,
			&c.token,
			"GET",
			&format!("/api/runs/{}/working-area", run.id),
			Value::Null
		)
		.await
		.0 >= 400
	);
	let (status,restored)=request(&c.app,&c.token,"POST",&format!("/api/working-areas/{id}/restore"),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":cleaned["revision"],"snapshot_id":cleanup["operation_id"],"thread_id":area["thread_id"]})).await;
	assert_eq!(status, 200, "{restored}");
	assert_eq!(restored["manifest"][0]["digest"], digest);
	assert_ne!(
		restored["manifest"][0]["file_id"],
		area["manifest"][0]["file_id"]
	);
	let (status, bytes) = request(
		&c.app,
		&c.token,
		"GET",
		&format!(
			"/api/working-areas/{id}/files/{}/download",
			restored["manifest"][0]["file_id"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{bytes}");
	use base64::Engine;
	assert_eq!(
		base64::engine::general_purpose::STANDARD
			.decode(bytes["data"].as_str().unwrap())
			.unwrap(),
		"保存する東京の資料\n".as_bytes()
	);
	assert_eq!(request(&c.app,&c.token,"POST",&path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":restored["revision"],"choice":"irreversible"})).await.0,400);
	let (status, confirmation) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/working-areas/{id}/deletion-confirmation"),
		json!({"expected_revision":restored["revision"]}),
	)
	.await;
	assert_eq!(status, 200, "{confirmation}");
	let input = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":restored["revision"],"choice":"irreversible","confirmation_id":confirmation["confirmation_id"]});
	let (status, deletion) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{deletion}");
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
	loop {
		let (_, state) = request(
			&c.app,
			&c.token,
			"GET",
			&format!(
				"/api/file-cleanups/{}",
				deletion["operation_id"].as_str().unwrap()
			),
			Value::Null,
		)
		.await;
		if state["state"] == "deleted" {
			break;
		}
		assert!(tokio::time::Instant::now() < deadline, "{state}");
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	let mut stale = input;
	stale["idempotency_key"] = json!(Uuid::new_v4());
	assert_eq!(request(&c.app, &c.token, "POST", &path, stale).await.0, 409);
	assert_eq!(
		request(
			&c.app,
			&c.token,
			"GET",
			&format!(
				"/api/working-areas/{id}/files/{}/download",
				restored["manifest"][0]["file_id"].as_str().unwrap()
			),
			Value::Null
		)
		.await
		.0,
		409
	);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}

#[path = "core_capabilities/transfer.rs"]
mod transfer_tests;

#[path = "core_capabilities/lifecycle.rs"]
mod lifecycle_tests;

#[cfg(feature = "capability-runtime-tests")]
#[path = "core_capabilities/runtime_faults.rs"]
mod runtime_fault_tests;

#[cfg(feature = "capability-runtime-tests")]
#[path = "core_capabilities/packages.rs"]
mod package_tests;

#[path = "core_capabilities/configuration.rs"]
mod configuration_tests;

#[cfg(feature = "capability-runtime-tests")]
#[path = "core_capabilities/journey.rs"]
mod journey_tests;

#[cfg(feature = "capability-runtime-tests")]
#[path = "core_capabilities/reference_limits.rs"]
mod reference_limit_tests;

#[cfg(feature = "capability-runtime-tests")]
#[path = "core_capabilities/network.rs"]
mod network_tests;

#[path = "core_capabilities/session_boundaries.rs"]
mod session_boundary_tests;

#[path = "core_capabilities/content_limits.rs"]
mod content_limit_tests;

#[path = "core_capabilities/retention_boundaries.rs"]
mod retention_boundary_tests;

#[cfg(feature = "capability-runtime-tests")]
#[path = "core_capabilities/extraction_lifecycle.rs"]
mod extraction_lifecycle_tests;

#[path = "core_capabilities/subject_sessions.rs"]
mod subject_session_tests;

#[path = "core_capabilities/upload_bounds.rs"]
mod upload_bound_tests;

#[cfg(feature = "capability-runtime-tests")]
#[path = "core_capabilities/reference_dispatch.rs"]
mod reference_dispatch_tests;

#[cfg(feature = "capability-runtime-tests")]
#[path = "core_capabilities/publication_storage.rs"]
mod publication_storage_tests;
