mod common;

use aidash::federation::Federation;
use common::{TestEnvironment, cleanup, request, setup, test_environment};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use std::{
	collections::VecDeque,
	sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	},
	time::Duration,
};
use tokio::sync::Mutex;

struct Workbench {
	_environment: Arc<TestEnvironment>,
	f: Federation,
	app: axum::Router,
	url: String,
	schema: String,
	token: String,
	draft: Value,
	responses: Arc<Mutex<VecDeque<Value>>>,
	hits: Arc<AtomicUsize>,
	server: tokio::task::JoinHandle<()>,
	endpoint: String,
}

impl Workbench {
	fn path(&self) -> String {
		format!(
			"/api/workbench/drafts/{}",
			self.draft["id"].as_str().unwrap()
		)
	}

	async fn call(&self, method: &str, path: &str, body: Value) -> Value {
		let (status, result) = request(&self.app, &self.token, method, path, body).await;
		assert_eq!(status, 200, "{method} {path}: {result}");
		result
	}

	async fn register(&self) -> Value {
		self.call(
			"POST",
			&format!("{}/register", self.path()),
			json!({"expected_revision":1}),
		)
		.await
	}

	async fn finished(&self, input: Value) -> Value {
		let path = format!("{}/tests", self.path());
		let started = self.call("POST", &path, input).await;
		for _ in 0..200 {
			let sessions = self.call("GET", &path, Value::Null).await;
			let session = sessions
				.as_array()
				.unwrap()
				.iter()
				.find(|s| s["id"] == started["id"])
				.unwrap();
			if session["status"] != "running" {
				return session.clone();
			}
			tokio::time::sleep(Duration::from_millis(25)).await;
		}
		panic!("test session did not finish: {started}");
	}

	async fn cleanup(self) {
		self.server.abort();
		cleanup(self.f, &self.url, &self.schema).await;
	}
}

#[rstest::fixture]
async fn workbench() -> Workbench {
	let environment = test_environment().await;
	let (f, url, schema) = setup(&environment).await;
	let app = aidash::api::router(f.clone());
	let responses = Arc::new(Mutex::new(VecDeque::from([
		json!({"choices":[{"finish_reason":"stop","message":{"content":"Complete"}}],"usage":{"prompt_tokens":30,"completion_tokens":5}}),
	])));
	let hits = Arc::new(AtomicUsize::new(0));
	let queue = responses.clone();
	let count = hits.clone();
	let mock = axum::Router::new().route(
		"/v1/chat/completions",
		axum::routing::post(move || {
			let queue = queue.clone();
			let count = count.clone();
			async move {
				count.fetch_add(1, Ordering::SeqCst);
				let mut queue = queue.lock().await;
				let result = if queue.len() > 1 {
					queue.pop_front().unwrap()
				} else {
					queue.front().unwrap().clone()
				};
				axum::Json(result)
			}
		}),
	);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener, mock).await.unwrap();
	});
	for (id, kind, config) in [
		(
			"fixture-model",
			"model",
			json!({"provider":"openrouter","model_id":"fixture","endpoint":format!("{endpoint}/v1"),"credential_env":null,"context_window":32768,"max_output_tokens":2048,"modalities":["text"],"cost":{}}),
		),
		(
			"fixture-tool",
			"tool",
			json!({"transport":"http","endpoint":format!("{endpoint}/effect"),"credential_env":null,"replay":"read_only"}),
		),
	] {
		assert_eq!(request(&app, &f.config.api_token, "POST", "/api/registry", json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"Fixture"},"config":config})).await.0, 200);
	}
	assert_eq!(request(&app, &f.config.api_token, "POST", "/api/authorization/acme", json!({"expected_revision":0,"bundle":{"tenant":"acme","subjects":{"alice":{"kind":"user"},"bob":{"kind":"user"}},"policies":[{"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]}})).await.0, 200);
	let (status, credential) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	assert_eq!(status, 200);
	let token = credential["token"].as_str().unwrap().to_owned();
	let (status, draft) = request(&app, &token, "POST", "/api/workbench/drafts", json!({"entry":{"id":"","version":"1.0.0","kind":"agent","name":{"en":"Regression fixture"},"description":{"en":"Fixture"},"config":{"model":{"id":"fixture-model","version":"1.0.0"},"instructions":"Summarize","tools":[{"id":"fixture-tool","version":"1.0.0"}],"skills":[],"cluster":null,"max_steps":8}}})).await;
	assert_eq!(status, 200, "draft: {draft}");
	Workbench {
		_environment: environment,
		f,
		app,
		url,
		schema,
		token,
		draft,
		responses,
		hits,
		server,
		endpoint,
	}
}

#[rstest::fixture]
async fn crowded_workbench() -> Workbench {
	let wb = workbench().await;
	// An older shared draft must also survive the invisible newer batch.
	let (status, shared) = request(&wb.app, &wb.f.config.api_token, "POST", "/api/workbench/drafts", json!({"tenant":"acme","owner":"bob","entry":{
		"id":"","version":"1.0.0","kind":"agent","name":{"en":"Shared fixture"},"description":{"en":"Fixture"},"config":wb.draft["entry"]["config"]
	}})).await;
	assert_eq!(status, 200, "shared draft: {shared}");
	assert_eq!(
		request(
			&wb.app,
			&wb.f.config.api_token,
			"POST",
			&format!(
				"/api/workbench/drafts/{}/shares",
				shared["id"].as_str().unwrap()
			),
			json!({"subject":"alice","can_edit":false,"enabled":true,"include_documents":false})
		)
		.await
		.0,
		200
	);
	let query = Query::insert()
		.into_table(Alias::new("agent_drafts"))
		.columns(["id", "tenant", "owner", "managed_id", "entry", "documents"].map(Alias::new))
		.values_panic([
			Expr::cust("$1"),
			Expr::value("acme"),
			Expr::value("bob"),
			Expr::cust("$2"),
			Expr::cust("$3"),
			Expr::cust("'[]'::jsonb"),
		])
		.to_string(PostgresQueryBuilder);
	for _ in 0..101 {
		let id = uuid::Uuid::now_v7();
		let mut entry = wb.draft["entry"].clone();
		entry["id"] = json!(id);
		sqlx::query(&query)
			.bind(id)
			.bind(id.to_string())
			.bind(entry)
			.execute(&wb.f.store.pool)
			.await
			.unwrap();
	}
	wb
}

#[rstest::rstest]
#[tokio::test]
async fn draft_list_reaches_authorized_rows_beyond_invisible_batch(
	#[future(awt)] crowded_workbench: Workbench,
) {
	let wb = crowded_workbench;
	let visible = wb.call("GET", "/api/workbench/drafts", Value::Null).await;
	assert_eq!(visible.as_array().unwrap().len(), 2, "visible: {visible}");
	assert!(
		visible
			.as_array()
			.unwrap()
			.iter()
			.any(|row| row["id"] == wb.draft["id"])
	);
	assert!(
		visible
			.as_array()
			.unwrap()
			.iter()
			.any(|row| row["owner"] == "bob")
	);
	wb.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn duplicate_waits_for_concurrent_source_edit_and_rejects_stale_revision(
	#[future(awt)] workbench: Workbench,
) {
	let wb = workbench;
	let mut edit = wb.f.store.pool.begin().await.unwrap();
	let id: uuid::Uuid = wb.draft["id"].as_str().unwrap().parse().unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("agent_drafts"))
			.value(Alias::new("revision"), Expr::cust("revision + 1"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.execute(&mut *edit)
	.await
	.unwrap();
	let app = wb.app.clone();
	let token = wb.token.clone();
	let path = format!("{}/duplicate", wb.path());
	let mut duplicate = tokio::spawn(async move {
		request(&app, &token, "POST", &path, json!({"expected_revision":1})).await
	});
	let before_commit = tokio::time::timeout(Duration::from_millis(200), &mut duplicate).await;
	edit.commit().await.unwrap();
	assert!(
		before_commit.is_err(),
		"duplicate copied an unlocked stale source: {before_commit:?}"
	);
	let (status, result) = duplicate.await.unwrap();
	assert_eq!(status, 409, "duplicate: {result}");
	wb.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn registration_drops_knowledge_digest_after_documents_are_removed(
	#[future(awt)] workbench: Workbench,
) {
	let wb = workbench;
	let mut entry = wb.draft["entry"].clone();
	entry["config"]["knowledge_digest"] = json!("a".repeat(64));
	let saved = wb.call("PUT", &wb.path(), json!({"expected_revision":1,"entry":entry,"documents":[{"name":"notes.txt","media_type":"text/plain","text":"Private reference"}]})).await;
	let cleared = wb
		.call(
			"PUT",
			&wb.path(),
			json!({"expected_revision":2,"entry":saved["entry"],"documents":[]}),
		)
		.await;
	assert_eq!(cleared["revision"], 3);
	let registered = wb
		.call(
			"POST",
			&format!("{}/register", wb.path()),
			json!({"expected_revision":3}),
		)
		.await;
	assert!(
		registered["entry"]["config"]
			.get("knowledge_digest")
			.is_none(),
		"registration: {registered}"
	);
	wb.cleanup().await;
}

#[rstest::rstest]
#[case(json!(null))]
#[case(json!({"prompt_tokens":30}))]
#[case(json!({"completion_tokens":5}))]
#[tokio::test]
async fn incomplete_usage_blocks_test_before_tool_or_second_inference(
	#[future(awt)] workbench: Workbench,
	#[case] usage: Value,
) {
	let wb = workbench;
	*wb.responses.lock().await = VecDeque::from([
		json!({"choices":[{"finish_reason":"tool_calls","message":{"content":"","tool_calls":[{"id":"call-1","function":{"name":"plugin_0","arguments":"{}"}}]}}],"usage":usage}),
		json!({"choices":[{"finish_reason":"stop","message":{"content":"Complete"}}],"usage":{"prompt_tokens":30,"completion_tokens":5}}),
	]);
	let session = wb.finished(json!({"expected_revision":1,"message":"Use the tool","fixtures":{"plugin_0":{"status":"success","response":{"ok":true}}}})).await;
	assert_eq!(session["status"], "blocked", "session: {session}");
	assert_eq!(session["usage"]["usage_complete"], false);
	assert!(session["error"].as_str().unwrap().contains("usage"));
	assert_eq!(session["tool_calls"], json!([]));
	assert_eq!(wb.hits.load(Ordering::SeqCst), 1);
	wb.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn expired_real_tests_keep_profile_and_continuation_metadata_in_trust(
	#[future(awt)] workbench: Workbench,
) {
	let wb = workbench;
	assert_eq!(request(&wb.app, &wb.f.config.api_token, "PUT", "/api/workbench/test-profiles/acme/sandbox", json!({"expected_revision":0,"enabled":true,"rules":[{"tool":{"id":"fixture-tool","version":"1.0.0"},"endpoint":format!("{}/test-effect", wb.endpoint),"credential_env":null,"allowed_actions":["read"],"allowed_resources":["sandbox"]}]})).await.0, 200);
	let first = wb
		.finished(
			json!({"expected_revision":1,"message":"First","mode":"real","profile_id":"sandbox"}),
		)
		.await;
	assert_eq!(first["status"], "completed", "first: {first}");
	let second = wb.finished(json!({"expected_revision":1,"message":"Continue","mode":"real","profile_id":"sandbox","continue_from":first["id"],"fixtures":{"private_fixture":{"status":"success","response":{"private":"payload"}}}})).await;
	assert_eq!(second["status"], "completed", "second: {second}");
	wb.register().await;
	sqlx::query(
		&Query::update()
			.table(Alias::new("agent_test_sessions"))
			.value(
				Alias::new("expires_at"),
				Expr::cust("clock_timestamp() - interval '1 day'"),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&wb.f.store.pool)
	.await
	.unwrap();
	assert_eq!(
		aidash::workbench::purge_expired(&wb.f.store.pool)
			.await
			.unwrap(),
		2
	);
	let sessions = wb
		.call("GET", &format!("{}/tests", wb.path()), Value::Null)
		.await;
	let continued = sessions
		.as_array()
		.unwrap()
		.iter()
		.find(|s| s["id"] == second["id"])
		.unwrap();
	assert_eq!(continued["scenario"]["mode"], "real");
	assert_eq!(continued["scenario"]["profile_id"], "sandbox");
	assert_eq!(continued["scenario"]["profile_revision"], 1);
	assert_eq!(continued["scenario"]["continue_from"], first["id"]);
	assert!(continued["scenario"].get("fixtures").is_none());
	assert_eq!(continued["conversation"], Value::Null);
	assert_eq!(continued["tool_calls"], Value::Null);
	let inspection = wb
		.call(
			"GET",
			&format!(
				"/api/workbench/versions/{}/1.0.0",
				wb.draft["entry"]["id"].as_str().unwrap()
			),
			Value::Null,
		)
		.await;
	assert_eq!(inspection["test_evidence"].as_array().unwrap().len(), 2);
	for evidence in inspection["test_evidence"].as_array().unwrap() {
		assert_eq!(evidence["mode"], "real");
		assert_eq!(evidence["profile_id"], "sandbox");
		assert_eq!(evidence["profile_revision"], 1);
		assert!(evidence["expired_at"].is_string());
	}
	wb.cleanup().await;
}

#[rstest::fixture]
async fn expired_incidents() -> Workbench {
	let wb = workbench().await;
	wb.register().await;
	let path = format!(
		"/api/workbench/versions/{}/1.0.0/incidents",
		wb.draft["entry"]["id"].as_str().unwrap()
	);
	for _ in 0..101 {
		wb.call("POST", &path, json!({"severity":"medium","owner":"alice","notes":"Fixture","evidence":[{"title":"Copy","content":"Retained payload"}]})).await;
	}
	sqlx::query(
		&Query::update()
			.table(Alias::new("agent_incidents"))
			.value(Alias::new("status"), Expr::value("resolved"))
			.value(
				Alias::new("evidence_expires_at"),
				Expr::cust("clock_timestamp() - interval '1 day'"),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&wb.f.store.pool)
	.await
	.unwrap();
	wb
}

#[rstest::rstest]
#[tokio::test]
async fn incident_expiry_drains_multiple_batches_in_one_invocation(
	#[future(awt)] expired_incidents: Workbench,
) {
	let wb = expired_incidents;
	assert_eq!(
		aidash::workbench::purge_incident_evidence(&wb.f.store.pool)
			.await
			.unwrap(),
		101
	);
	let copies: Vec<Value> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("evidence"))
			.from(Alias::new("agent_incidents"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&wb.f.store.pool)
	.await
	.unwrap();
	assert_eq!(copies.len(), 101);
	assert!(
		copies
			.iter()
			.all(|copy| copy[0]["content"].is_null() && copy[0]["sha256"].is_string())
	);
	assert_eq!(
		aidash::workbench::purge_incident_evidence(&wb.f.store.pool)
			.await
			.unwrap(),
		0
	);
	wb.cleanup().await;
}

#[rstest::rstest]
#[case(true, false)]
#[case(false, true)]
#[case(true, true)]
#[tokio::test]
async fn trust_permissions_use_worker_transport_and_registry_read(
	#[future(awt)] workbench: Workbench,
	#[case] worker_allowed: bool,
	#[case] registry_allowed: bool,
) {
	let wb = workbench;
	let registered = wb.register().await;
	for entry in [
		&registered["entry"],
		&json!({"id":"fixture-model","version":"1.0.0"}),
	] {
		assert_eq!(request(&wb.app, &wb.f.config.api_token, "POST", "/api/authorization/acme/catalog", json!({"entry":{"id":entry["id"],"version":entry["version"]},"enabled":true,"expected_revision":0})).await.0, 200);
	}
	let mut policies = vec![
		json!({"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}),
	];
	if registry_allowed {
		policies.push(json!({"id":"transport","effect":"deny","subjects":{"any":true},"actions":["model.infer"],"resources":{"kinds":["model"]},"condition":{"op":"eq","left":{"source":"environment","path":"/transport"},"right":{"source":"literal","value":if worker_allowed { "api" } else { "worker" }}}}));
	}
	if !registry_allowed {
		policies.push(json!({"id":"registry","effect":"deny","subjects":{"any":true},"actions":["registry.read"],"resources":{"kinds":["model"]}}));
	}
	assert_eq!(request(&wb.app, &wb.f.config.api_token, "POST", "/api/authorization/acme", json!({"expected_revision":1,"bundle":{"tenant":"acme","subjects":{"alice":{"kind":"user"},"bob":{"kind":"user"}},"policies":policies}})).await.0, 200);
	let path = format!(
		"/api/workbench/versions/{}/1.0.0/permissions",
		wb.draft["entry"]["id"].as_str().unwrap()
	);
	let context = wb
		.call("POST", &path, json!({"tenant":"acme","subject":"alice"}))
		.await;
	let model = context["rows"]
		.as_array()
		.unwrap()
		.iter()
		.find(|row| row["kind"] == "model")
		.unwrap();
	assert_eq!(
		model["policy_allowed"], worker_allowed,
		"context: {context}"
	);
	assert_eq!(
		model["registry_read_allowed"], registry_allowed,
		"context: {context}"
	);
	assert_eq!(
		model["effective_for_component"],
		worker_allowed && registry_allowed
	);
	wb.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn workbench_rollback_preserves_registered_and_packaged_behavior_flags(
	#[future(awt)] workbench: Workbench,
) {
	use migration::MigratorTrait;
	let wb = workbench;
	let registered = wb.register().await;
	let entry: aidash::registry::Entry =
		serde_json::from_value(registered["entry"].clone()).unwrap();
	let package =
		wb.f.registry
			.publish(
				&wb.f.store.pool,
				aidash::registry::Package {
					entity: entry.clone(),
					author: "Fixture".into(),
					permissions: vec![],
					dependencies: vec![],
				},
			)
			.await
			.unwrap();
	let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(wb.f.store.pool.clone());
	common::rollback_from_migration(&db, "m20260925_000000_agent_workbenches").await;
	assert_eq!(
		wb.f.registry.get(&entry.id, &entry.version).await.unwrap(),
		entry
	);
	let manifest: Value = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("manifest"))
			.from(Alias::new("packages"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&wb.f.store.pool)
	.await
	.unwrap();
	assert_eq!(manifest, package.manifest);
	migration::Migrator::up(&db, None).await.unwrap();
	assert_eq!(
		wb.f.registry.get(&entry.id, &entry.version).await.unwrap(),
		entry
	);
	wb.cleanup().await;
}
