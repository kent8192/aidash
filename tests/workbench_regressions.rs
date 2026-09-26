mod common;

use aidash::federation::Federation;
use axum::response::IntoResponse;
use common::{TestEnvironment, cleanup, request, setup, test_environment};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use std::{
	collections::VecDeque,
	sync::{
		Arc,
		atomic::{AtomicBool, AtomicUsize, Ordering},
	},
	time::Duration,
};
use tokio::sync::{Mutex, Notify};

#[derive(Default)]
struct ModelGate {
	paused: AtomicBool,
	arrived: Notify,
	release: Notify,
}

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
	model_gate: Arc<ModelGate>,
	effect_gate: Arc<ModelGate>,
	effect_hits: Arc<AtomicUsize>,
	effect_reply_invalid: Arc<AtomicBool>,
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

	async fn operator_finished(&self, started: &Value) -> Value {
		let mut finished = Value::Null;
		for _ in 0..200 {
			let (status, sessions) = request(
				&self.app,
				&self.f.config.api_token,
				"GET",
				&format!("{}/tests", self.path()),
				Value::Null,
			)
			.await;
			assert_eq!(status, 200, "sessions: {sessions}");
			let session = sessions
				.as_array()
				.unwrap()
				.iter()
				.find(|s| s["id"] == started["id"])
				.unwrap();
			if session["status"] != "running" {
				finished = session.clone();
				break;
			}
			tokio::time::sleep(Duration::from_millis(25)).await;
		}
		finished
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
	let model_gate = Arc::new(ModelGate::default());
	let gate = model_gate.clone();
	let effect_gate = Arc::new(ModelGate::default());
	let dispatch_gate = effect_gate.clone();
	let effect_hits = Arc::new(AtomicUsize::new(0));
	let effects = effect_hits.clone();
	let effect_reply_invalid = Arc::new(AtomicBool::new(false));
	let corrupt_reply = effect_reply_invalid.clone();
	let queue = responses.clone();
	let count = hits.clone();
	let mock = axum::Router::new()
		.route(
			"/v1/chat/completions",
			axum::routing::post(move || {
				let queue = queue.clone();
				let count = count.clone();
				let gate = gate.clone();
				async move {
					count.fetch_add(1, Ordering::SeqCst);
					let mut queue = queue.lock().await;
					let result = if queue.len() > 1 {
						queue.pop_front().unwrap()
					} else {
						queue.front().unwrap().clone()
					};
					drop(queue);
					if gate.paused.load(Ordering::SeqCst) {
						gate.arrived.notify_one();
						gate.release.notified().await;
					}
					axum::Json(result)
				}
			}),
		)
		.route(
			"/test-effect",
			axum::routing::post(move || {
				let effects = effects.clone();
				let corrupt_reply = corrupt_reply.clone();
				let gate = dispatch_gate.clone();
				async move {
					effects.fetch_add(1, Ordering::SeqCst);
					if gate.paused.load(Ordering::SeqCst) {
						gate.arrived.notify_one();
						gate.release.notified().await;
					}
					if corrupt_reply.load(Ordering::SeqCst) {
						"{".into_response()
					} else {
						axum::Json(json!({"ok":true})).into_response()
					}
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
		model_gate,
		effect_gate,
		effect_hits,
		effect_reply_invalid,
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

#[rstest::fixture]
async fn revoked_workbench() -> (Workbench, axum::Router) {
	let wb = workbench().await;
	let authorization = aidash::authorization::Authorization {
		pool: wb.f.store.pool.clone(),
	};
	let actor = authorization.authenticate(&wb.token).await.unwrap();
	let credential = authorization.credentials("acme").await.unwrap().remove(0);
	authorization
		.revoke_credential("acme", credential.id)
		.await
		.unwrap();
	// Capture the authenticated actor before revocation, as middleware can do.
	let (routes, _) = aidash::workbench::routes().split_for_parts();
	let app = axum::Router::new()
		.nest("/api", routes)
		.layer(axum::Extension(actor))
		.with_state(wb.f.clone());
	(wb, app)
}

#[rstest::rstest]
#[tokio::test]
async fn draft_mutation_rechecks_the_authenticated_credential_before_commit(
	#[future(awt)] revoked_workbench: (Workbench, axum::Router),
) {
	let (wb, app) = revoked_workbench;
	let (status, result) = request(&app, &wb.token, "PUT", &wb.path(), json!({"expected_revision":1,"entry":wb.draft["entry"],"documents":[],"release_notes":"Must not commit"})).await;
	assert_eq!(status, 401, "revoked actor: {result}");
	let unchanged = request(
		&wb.app,
		&wb.f.config.api_token,
		"GET",
		&wb.path(),
		Value::Null,
	)
	.await
	.1;
	assert_eq!(unchanged["revision"], 1);
	assert_eq!(unchanged["release_notes"], "");
	wb.cleanup().await;
}

#[rstest::fixture]
async fn clustered_workbench() -> Workbench {
	let mut wb = workbench().await;
	wb.register().await;
	let (status, cluster) = request(&wb.app, &wb.f.config.api_token, "POST", "/api/registry", json!({"id":"fixture-cluster","version":"1.0.0","kind":"cluster","name":{"en":"Fixture cluster"},"description":{"en":"Fixture"},"config":{"coordinator":{"id":wb.draft["entry"]["id"],"version":"1.0.0"}}})).await;
	assert_eq!(status, 200, "cluster: {cluster}");
	let mut entry = wb.draft["entry"].clone();
	entry["version"] = json!("1.0.1");
	entry["config"]["cluster"] = json!({"id":cluster["id"],"version":cluster["version"]});
	wb.draft = wb
		.call(
			"PUT",
			&wb.path(),
			json!({"expected_revision":1,"entry":entry,"documents":[]}),
		)
		.await;
	wb.call(
		"POST",
		&format!("{}/register", wb.path()),
		json!({"expected_revision":2}),
	)
	.await;
	for reference in [
		json!({"id":wb.draft["entry"]["id"],"version":"1.0.1"}),
		json!({"id":"fixture-model","version":"1.0.0"}),
		entry["config"]["cluster"].clone(),
	] {
		assert_eq!(
			request(
				&wb.app,
				&wb.f.config.api_token,
				"POST",
				"/api/authorization/acme/catalog",
				json!({"entry":reference,"enabled":true,"expected_revision":0})
			)
			.await
			.0,
			200
		);
	}
	wb
}

#[rstest::rstest]
#[case("cluster.execute")]
#[case("registry.read")]
#[tokio::test]
async fn trust_reports_the_configured_cluster_and_its_execution_dependencies(
	#[future(awt)] clustered_workbench: Workbench,
	#[case] denied_action: &str,
) {
	let wb = clustered_workbench;
	assert_eq!(request(&wb.app, &wb.f.config.api_token, "POST", "/api/authorization/acme", json!({"expected_revision":1,"bundle":{"tenant":"acme","subjects":{"alice":{"kind":"user"},"bob":{"kind":"user"}},"policies":[{"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},{"id":"cluster-denial","effect":"deny","subjects":{"any":true},"actions":[denied_action],"resources":{"kinds":["cluster"]}}]}})).await.0, 200);
	let context = wb
		.call(
			"POST",
			&format!(
				"/api/workbench/versions/{}/1.0.1/permissions",
				wb.draft["entry"]["id"].as_str().unwrap()
			),
			json!({"tenant":"acme","subject":"alice"}),
		)
		.await;
	let cluster = context["rows"]
		.as_array()
		.unwrap()
		.iter()
		.find(|row| row["kind"] == "cluster")
		.expect("configured cluster must appear in Trust");
	assert_eq!(cluster["action"], "cluster.execute");
	assert_eq!(cluster["catalog_enabled"], true);
	assert_eq!(
		cluster["registry_read_allowed"],
		denied_action != "registry.read"
	);
	assert_eq!(
		cluster["policy_allowed"],
		denied_action != "cluster.execute"
	);
	assert_eq!(cluster["effective_for_component"], false);
	wb.cleanup().await;
}

#[rstest::fixture]
async fn hidden_incident_backlog(#[default(false)] other_tenant: bool) -> (Workbench, Value) {
	let wb = workbench().await;
	wb.register().await;
	let path = format!(
		"/api/workbench/versions/{}/1.0.0/incidents",
		wb.draft["entry"]["id"].as_str().unwrap()
	);
	let older = wb
		.call(
			"POST",
			&path,
			json!({"severity":"medium","owner":"alice","notes":"Older authorized incident"}),
		)
		.await;
	if other_tenant {
		assert_eq!(request(&wb.app, &wb.f.config.api_token, "POST", "/api/authorization/other", json!({"expected_revision":0,"bundle":{"tenant":"other","subjects":{"bob":{"kind":"user"}},"policies":[{"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]}})).await.0, 200);
	}
	let mut hidden = Vec::new();
	for _ in 0..101 {
		let (status, incident) = request(&wb.app, &wb.f.config.api_token, "POST", &path, json!({"tenant":if other_tenant {"other"} else {"acme"},"severity":"medium","owner":"bob","notes":"Inaccessible incident"})).await;
		assert_eq!(status, 200, "incident: {incident}");
		hidden.push(incident["id"].clone());
	}
	if !other_tenant {
		assert_eq!(request(&wb.app, &wb.f.config.api_token, "POST", "/api/authorization/acme", json!({"expected_revision":1,"bundle":{"tenant":"acme","subjects":{"alice":{"kind":"user"},"bob":{"kind":"user"}},"policies":[{"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},{"id":"incident-denial","effect":"deny","subjects":{"any":true},"actions":["agent_incident.read"],"resources":{"kinds":["agent_incident"],"ids":hidden}}]}})).await.0, 200);
	}
	(wb, older)
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn incident_list_reaches_older_authorized_rows(
	#[case] _other_tenant: bool,
	#[future(awt)]
	#[with(_other_tenant)]
	hidden_incident_backlog: (Workbench, Value),
) {
	let (wb, older) = hidden_incident_backlog;
	let visible = wb
		.call(
			"GET",
			&format!(
				"/api/workbench/versions/{}/1.0.0/incidents",
				wb.draft["entry"]["id"].as_str().unwrap()
			),
			Value::Null,
		)
		.await;
	assert_eq!(visible.as_array().unwrap().len(), 1, "incidents: {visible}");
	assert_eq!(visible[0]["id"], older["id"]);
	wb.cleanup().await;
}

#[rstest::fixture]
async fn model_waiting_for_real_tool(#[default(false)] shared: bool) -> (Workbench, Value) {
	let wb = workbench().await;
	*wb.responses.lock().await = VecDeque::from([
		json!({"choices":[{"finish_reason":"tool_calls","message":{"content":"","tool_calls":[{"id":"real-call","function":{"name":"plugin_0","arguments":"{\"action\":\"read\",\"resource\":\"sandbox\"}"}}]}}],"usage":{"prompt_tokens":30,"completion_tokens":5}}),
	]);
	assert_eq!(request(&wb.app, &wb.f.config.api_token, "PUT", "/api/workbench/test-profiles/acme/sandbox", json!({"expected_revision":0,"enabled":true,"rules":[{"tool":{"id":"fixture-tool","version":"1.0.0"},"endpoint":format!("{}/test-effect",wb.endpoint),"credential_env":null,"allowed_actions":["read"],"allowed_resources":["sandbox"]}]})).await.0, 200);
	let token = if shared {
		wb.call(
			"POST",
			&format!("{}/shares", wb.path()),
			json!({"subject":"bob","can_edit":true,"enabled":true,"include_documents":false}),
		)
		.await;
		let (_, credential) = request(
			&wb.app,
			&wb.f.config.api_token,
			"POST",
			"/api/authorization/acme/credentials",
			json!({"subject":"bob"}),
		)
		.await;
		credential["token"].as_str().unwrap().to_owned()
	} else {
		wb.token.clone()
	};
	wb.model_gate.paused.store(true, Ordering::SeqCst);
	let (status, session) = request(
		&wb.app,
		&token,
		"POST",
		&format!("{}/tests", wb.path()),
		json!({"expected_revision":1,"message":"Use the tool","mode":"real","profile_id":"sandbox"}),
	)
	.await;
	assert_eq!(status, 200, "session: {session}");
	tokio::time::timeout(Duration::from_secs(5), wb.model_gate.arrived.notified())
		.await
		.expect("model fixture must reach the in-flight boundary");
	(wb, session)
}

#[rstest::rstest]
#[case("profile")]
#[case("credential")]
#[case("policy")]
#[tokio::test]
async fn pre_dispatch_revocation_is_denied_without_an_unknown_external_outcome(
	#[future(awt)] model_waiting_for_real_tool: (Workbench, Value),
	#[case] revoked: &str,
) {
	let (wb, started) = model_waiting_for_real_tool;
	match revoked {
		"profile" => assert_eq!(request(&wb.app, &wb.f.config.api_token, "PUT", "/api/workbench/test-profiles/acme/sandbox", json!({"expected_revision":1,"enabled":false,"rules":[]})).await.0, 200),
		"credential" => {
			let authorization = aidash::authorization::Authorization {pool:wb.f.store.pool.clone()};
			let credential = authorization.credentials("acme").await.unwrap().remove(0);
			authorization.revoke_credential("acme", credential.id).await.unwrap();
		}
		"policy" => assert_eq!(request(&wb.app, &wb.f.config.api_token, "POST", "/api/authorization/acme", json!({"expected_revision":1,"bundle":{"tenant":"acme","subjects":{"alice":{"kind":"user"},"bob":{"kind":"user"}},"policies":[{"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},{"id":"test-denial","effect":"deny","subjects":{"any":true},"actions":["agent_draft.test"],"resources":{"kinds":["agent_draft"]}}]}})).await.0, 200),
		_ => unreachable!(),
	}
	wb.model_gate.release.notify_one();
	let finished = wb.operator_finished(&started).await;
	assert_eq!(wb.effect_hits.load(Ordering::SeqCst), 0);
	assert_eq!(
		finished["status"], "blocked",
		"pre-dispatch {revoked}: {finished}"
	);
	let calls = finished["tool_calls"].as_array().unwrap();
	assert_eq!(calls.len(), 1);
	assert_eq!(calls[0]["outcome"], "denied");
	wb.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn an_unreadable_response_after_dispatch_preserves_the_unknown_external_outcome(
	#[future(awt)] model_waiting_for_real_tool: (Workbench, Value),
) {
	let (wb, started) = model_waiting_for_real_tool;
	wb.effect_reply_invalid.store(true, Ordering::SeqCst);
	wb.model_gate.release.notify_one();
	let finished = wb.operator_finished(&started).await;
	assert_eq!(wb.effect_hits.load(Ordering::SeqCst), 1);
	assert_eq!(finished["status"], "outcome_unknown", "session: {finished}");
	assert_eq!(finished["tool_calls"][0]["outcome"], "outcome_unknown");
	wb.cleanup().await;
}

#[rstest::fixture]
async fn production_endpoint_workbench() -> Workbench {
	let wb = workbench().await;
	assert_eq!(request(&wb.app, &wb.f.config.api_token, "POST", "/api/registry", json!({"id":"production-tool","version":"1.0.0","kind":"tool","name":{"en":"Production"},"description":{"en":"Fixture"},"config":{"transport":"http","endpoint":"https://example.com/api","credential_env":null,"replay":"read_only"}})).await.0, 200);
	wb
}

#[rstest::rstest]
#[case("https://EXAMPLE.com/api", 400)]
#[case("https://example.com:443/api", 400)]
#[case("https://example.com/other/../api", 400)]
#[case("https://example.com/test", 200)]
#[tokio::test]
async fn real_test_profiles_require_a_distinct_canonical_destination(
	#[future(awt)] production_endpoint_workbench: Workbench,
	#[case] endpoint: &str,
	#[case] expected: u16,
) {
	let wb = production_endpoint_workbench;
	let (status, body) = request(&wb.app, &wb.f.config.api_token, "PUT", "/api/workbench/test-profiles/acme/canonical", json!({"expected_revision":0,"enabled":true,"rules":[{"tool":{"id":"production-tool","version":"1.0.0"},"endpoint":endpoint,"credential_env":null,"allowed_actions":["read"],"allowed_resources":["sandbox"]}]})).await;
	assert_eq!(status, expected, "profile: {body}");
	wb.cleanup().await;
}

#[rstest::fixture]
async fn one_step_workbench() -> Workbench {
	let mut wb = workbench().await;
	let mut entry = wb.draft["entry"].clone();
	entry["config"]["max_steps"] = json!(1);
	wb.draft = wb
		.call(
			"PUT",
			&wb.path(),
			json!({"expected_revision":1,"entry":entry,"documents":[]}),
		)
		.await;
	*wb.responses.lock().await = VecDeque::from([
		json!({"choices":[{"finish_reason":"tool_calls","message":{"content":"","tool_calls":[{"id":"first-step","function":{"name":"plugin_0","arguments":"{}"}}]}}],"usage":{"prompt_tokens":30,"completion_tokens":5}}),
		json!({"choices":[{"finish_reason":"stop","message":{"content":"Complete"}}],"usage":{"prompt_tokens":30,"completion_tokens":5}}),
	]);
	wb
}

#[rstest::rstest]
#[tokio::test]
async fn behavioral_evidence_cannot_exceed_the_draft_agents_step_limit(
	#[future(awt)] one_step_workbench: Workbench,
) {
	let wb = one_step_workbench;
	let session = wb.finished(json!({"expected_revision":2,"message":"Use the tool","fixtures":{"plugin_0":{"status":"success","response":{"ok":true}}}})).await;
	assert_eq!(session["status"], "blocked", "session: {session}");
	assert_eq!(wb.hits.load(Ordering::SeqCst), 1);
	let registration = wb
		.call(
			"POST",
			&format!("{}/register", wb.path()),
			json!({"expected_revision":2}),
		)
		.await;
	assert_eq!(registration["behavioral_tested"], false);
	wb.cleanup().await;
}

#[rstest::fixture]
async fn incident_event_backlog() -> (Workbench, Value) {
	let wb = workbench().await;
	wb.register().await;
	let incident = wb
		.call(
			"POST",
			&format!(
				"/api/workbench/versions/{}/1.0.0/incidents",
				wb.draft["entry"]["id"].as_str().unwrap()
			),
			json!({"severity":"medium","owner":"alice","notes":"History"}),
		)
		.await;
	let query = Query::insert()
		.into_table(Alias::new("agent_incident_events"))
		.columns(["incident_id", "actor", "change"].map(Alias::new))
		.values_panic([Expr::cust("$1"), Expr::value("alice"), Expr::cust("$2")])
		.to_string(PostgresQueryBuilder);
	for n in 0..501 {
		sqlx::query(&query)
			.bind(uuid::Uuid::parse_str(incident["id"].as_str().unwrap()).unwrap())
			.bind(json!({"sequence":n}))
			.execute(&wb.f.store.pool)
			.await
			.unwrap();
	}
	(wb, incident)
}

#[rstest::rstest]
#[tokio::test]
async fn incident_history_keeps_the_latest_events_in_display_order(
	#[future(awt)] incident_event_backlog: (Workbench, Value),
) {
	let (wb, incident) = incident_event_backlog;
	let events = wb
		.call(
			"GET",
			&format!(
				"/api/workbench/incidents/{}/events",
				incident["id"].as_str().unwrap()
			),
			Value::Null,
		)
		.await;
	let events = events.as_array().unwrap();
	assert_eq!(events.len(), 500);
	assert_eq!(events.first().unwrap()["change"]["sequence"], 1);
	assert_eq!(events.last().unwrap()["change"]["sequence"], 500);
	assert!(
		events
			.windows(2)
			.all(|rows| rows[0]["id"].as_i64().unwrap() < rows[1]["id"].as_i64().unwrap())
	);
	wb.cleanup().await;
}

#[rstest::fixture]
async fn trust_run_backlog(#[default("workspace.read")] denied_action: &str) -> (Workbench, Value) {
	let wb = workbench().await;
	wb.register().await;
	let visible =
		wb.f.store
			.create_workspace("Visible history", "Fixture")
			.await
			.unwrap();
	let hidden =
		wb.f.store
			.create_workspace("Hidden history", "Fixture")
			.await
			.unwrap();
	for workspace in [&visible, &hidden] {
		let query = Query::insert()
			.into_table(Alias::new("authorization_workspaces"))
			.columns(["workspace_id", "tenant", "owner_subject"].map(Alias::new))
			.values_panic([Expr::cust("$1"), Expr::value("acme"), Expr::value("alice")])
			.to_string(PostgresQueryBuilder);
		sqlx::query(&query)
			.bind(workspace.id)
			.execute(&wb.f.store.pool)
			.await
			.unwrap();
	}
	let visible = serde_json::to_value(visible).unwrap();
	let hidden = serde_json::to_value(hidden).unwrap();
	let mut hidden_runs = Vec::new();
	for n in 0..502 {
		let workspace = uuid::Uuid::parse_str(if n == 0 {
			visible["id"].as_str().unwrap()
		} else {
			hidden["id"].as_str().unwrap()
		})
		.unwrap();
		let task =
			wb.f.store
				.create_task(
					workspace,
					&aidash::domain::NewTask {
						title: "History".into(),
						description: "Fixture".into(),
						requirements: json!({}),
						dependencies: vec![],
						parent_id: None,
					},
					"alice",
					None,
				)
				.await
				.unwrap();
		let query = Query::insert()
			.into_table(Alias::new("runs"))
			.columns(
				[
					"id",
					"task_id",
					"workspace_id",
					"home_node",
					"agent_id",
					"agent_version",
					"phase",
				]
				.map(Alias::new),
			)
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::cust("$3"),
				Expr::cust("$4"),
				Expr::cust("$5"),
				Expr::value("1.0.0"),
				Expr::value("COMPLETED"),
			])
			.to_string(PostgresQueryBuilder);
		let run_id = uuid::Uuid::now_v7();
		if n > 0 {
			hidden_runs.push(run_id);
		}
		sqlx::query(&query)
			.bind(run_id)
			.bind(task.id)
			.bind(workspace)
			.bind(&wb.f.config.node_id)
			.bind(wb.draft["entry"]["id"].as_str().unwrap())
			.execute(&wb.f.store.pool)
			.await
			.unwrap();
	}
	assert_eq!(request(&wb.app,&wb.f.config.api_token,"POST","/api/authorization/acme",json!({"expected_revision":1,"bundle":{"tenant":"acme","subjects":{"alice":{"kind":"user"},"bob":{"kind":"user"}},"policies":[{"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},{"id":"hidden-workspace","effect":"deny","subjects":{"any":true},"actions":[denied_action],"resources":{"kinds":[if denied_action == "workspace.read" {"workspace"} else {"run"}],"ids":if denied_action == "workspace.read" {json!([hidden["id"]])} else {json!(hidden_runs)}}}]}})).await.0,200);
	(wb, visible)
}

#[rstest::rstest]
#[case("workspace.read")]
#[case("run.read")]
#[tokio::test]
async fn trust_inspection_reaches_older_runs_in_authorized_workspaces(
	#[case] _denied_action: &str,
	#[future(awt)]
	#[with(_denied_action)]
	trust_run_backlog: (Workbench, Value),
) {
	let (wb, visible) = trust_run_backlog;
	let inspection = tokio::time::timeout(
		Duration::from_secs(20),
		wb.call(
			"GET",
			&format!(
				"/api/workbench/versions/{}/1.0.0",
				wb.draft["entry"]["id"].as_str().unwrap()
			),
			Value::Null,
		),
	)
	.await
	.expect("Trust inspection must not wait on its own audit lease");
	assert_eq!(
		inspection["workspaces"].as_array().unwrap().len(),
		1,
		"inspection: {inspection}"
	);
	assert_eq!(inspection["workspaces"][0]["workspace_id"], visible["id"]);
	assert_eq!(inspection["usage_truncated"], false);
	wb.cleanup().await;
}

#[rstest::fixture]
async fn oversized_agent_tool_workbench() -> Workbench {
	let wb = workbench().await;
	wb.register().await;
	assert_eq!(request(&wb.app,&wb.f.config.api_token,"POST","/api/registry",json!({"id":"oversized-agent-tool","version":"1.0.0","kind":"tool","name":{"en":"Delegate"},"description":{"en":"Fixture"},"schema":{"type":"object","description":"A".repeat(180_000)},"config":{"transport":"agent","node_id":wb.f.config.node_id,"agent":{"id":wb.draft["entry"]["id"],"version":"1.0.0"}}})).await.0,200);
	wb
}

#[rstest::rstest]
#[case(false, 200)]
#[case(true, 400)]
#[tokio::test]
async fn creator_prompt_validation_charges_only_enabled_agent_tools(
	#[future(awt)] oversized_agent_tool_workbench: Workbench,
	#[case] delegation: bool,
	#[case] expected: u16,
) {
	let wb = oversized_agent_tool_workbench;
	let mut entry = wb.draft["entry"].clone();
	entry["config"]["tools"] = json!([{"id":"oversized-agent-tool","version":"1.0.0"}]);
	entry["config"]["allow_task_delegation"] = json!(delegation);
	entry["version"] = json!("1.0.1");
	let saved = wb
		.call(
			"PUT",
			&wb.path(),
			json!({"expected_revision":1,"entry":entry,"documents":[]}),
		)
		.await;
	let validation = wb
		.call(
			"POST",
			&format!("{}/validate", wb.path()),
			json!({"expected_revision":saved["revision"]}),
		)
		.await;
	assert_eq!(validation["valid"], !delegation, "validation: {validation}");
	let (status, body) = request(
		&wb.app,
		&wb.token,
		"POST",
		&format!("{}/register", wb.path()),
		json!({"expected_revision":saved["revision"]}),
	)
	.await;
	assert_eq!(status, expected, "registration: {body}");
	wb.cleanup().await;
}

#[rstest::fixture]
async fn revoked_incident_workbench() -> (Workbench, axum::Router, Value) {
	let wb = workbench().await;
	wb.register().await;
	let incident = wb
		.call(
			"POST",
			&format!(
				"/api/workbench/versions/{}/1.0.0/incidents",
				wb.draft["entry"]["id"].as_str().unwrap()
			),
			json!({"severity":"low","owner":"alice","notes":"Original incident"}),
		)
		.await;
	let authorization = aidash::authorization::Authorization {
		pool: wb.f.store.pool.clone(),
	};
	let actor = authorization.authenticate(&wb.token).await.unwrap();
	let credential = authorization.credentials("acme").await.unwrap().remove(0);
	authorization
		.revoke_credential("acme", credential.id)
		.await
		.unwrap();
	let (routes, _) = aidash::workbench::routes().split_for_parts();
	let app = axum::Router::new()
		.nest("/api", routes)
		.layer(axum::Extension(actor))
		.with_state(wb.f.clone());
	(wb, app, incident)
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn incident_mutations_reject_a_revoked_authenticated_actor(
	#[future(awt)] revoked_incident_workbench: (Workbench, axum::Router, Value),
	#[case] update: bool,
) {
	let (wb, app, incident) = revoked_incident_workbench;
	let path = if update {
		format!(
			"/api/workbench/incidents/{}",
			incident["id"].as_str().unwrap()
		)
	} else {
		format!(
			"/api/workbench/versions/{}/1.0.0/incidents",
			wb.draft["entry"]["id"].as_str().unwrap()
		)
	};
	let (status, result) = request(&app, &wb.token, if update {"PUT"} else {"POST"}, &path, if update { json!({"expected_revision":1,"severity":"critical","status":"resolved","owner":"alice","notes":"Must not commit"}) } else { json!({"severity":"critical","owner":"alice","notes":"Must not commit"}) }).await;
	assert_eq!(status, 401, "stale incident actor: {result}");
	let (read_status, rows) = request(
		&wb.app,
		&wb.f.config.api_token,
		"GET",
		&format!(
			"/api/workbench/versions/{}/1.0.0/incidents",
			wb.draft["entry"]["id"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	assert_eq!(read_status, 200, "stored incidents: {rows}");
	assert_eq!(rows.as_array().unwrap().len(), 1);
	assert_eq!(rows[0]["notes"], "Original incident");
	assert_eq!(rows[0]["revision"], 1);
	wb.cleanup().await;
}

#[rstest::fixture]
async fn visible_draft_backlog() -> Workbench {
	let wb = workbench().await;
	let timestamp: chrono::DateTime<chrono::Utc> =
		wb.draft["updated_at"].as_str().unwrap().parse().unwrap();
	let query = Query::insert()
		.into_table(Alias::new("agent_drafts"))
		.columns(
			[
				"id",
				"tenant",
				"owner",
				"managed_id",
				"entry",
				"documents",
				"updated_at",
			]
			.map(Alias::new),
		)
		.values_panic([
			Expr::cust("$1"),
			Expr::value("acme"),
			Expr::value("alice"),
			Expr::cust("$2"),
			Expr::cust("$3"),
			Expr::cust("'[]'::jsonb"),
			Expr::cust("$4"),
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
			.bind(timestamp)
			.execute(&wb.f.store.pool)
			.await
			.unwrap();
	}
	wb
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn draft_pages_reach_older_visible_rows_without_duplicates(
	#[future(awt)] visible_draft_backlog: Workbench,
	#[case] operator: bool,
) {
	let wb = visible_draft_backlog;
	let token = if operator {
		&wb.f.config.api_token
	} else {
		&wb.token
	};
	let (status, first) =
		request(&wb.app, token, "GET", "/api/workbench/drafts", Value::Null).await;
	assert_eq!(status, 200);
	assert_eq!(first.as_array().unwrap().len(), 100);
	let last = first.as_array().unwrap().last().unwrap();
	let mut next = reqwest::Url::parse("http://fixture/api/workbench/drafts").unwrap();
	next.query_pairs_mut()
		.append_pair("before_updated_at", last["updated_at"].as_str().unwrap())
		.append_pair("before_id", last["id"].as_str().unwrap());
	let (status, second) = request(
		&wb.app,
		token,
		"GET",
		&format!("{}?{}", next.path(), next.query().unwrap()),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "second page: {second}");
	assert_eq!(second.as_array().unwrap().len(), 2);
	assert!(
		second
			.as_array()
			.unwrap()
			.iter()
			.any(|d| d["id"] == wb.draft["id"])
	);
	assert!(
		second.as_array().unwrap().iter().all(|d| !first
			.as_array()
			.unwrap()
			.iter()
			.any(|f| f["id"] == d["id"]))
	);
	for partial in [
		format!(
			"/api/workbench/drafts?before_id={}",
			last["id"].as_str().unwrap()
		),
		format!(
			"/api/workbench/drafts?before_updated_at={}",
			last["updated_at"].as_str().unwrap().replace('+', "%2B")
		),
	] {
		assert_eq!(
			request(&wb.app, token, "GET", &partial, Value::Null)
				.await
				.0,
			400
		);
	}
	wb.cleanup().await;
}

#[rstest::fixture]
async fn shared_transfer_workbench(#[default(false)] can_edit: bool) -> (Workbench, String) {
	let wb = workbench().await;
	wb.call(
		"POST",
		&format!("{}/shares", wb.path()),
		json!({"subject":"bob","can_edit":can_edit,"enabled":true,"include_documents":false}),
	)
	.await;
	let (status, credential) = request(
		&wb.app,
		&wb.f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	assert_eq!(status, 200);
	let token = credential["token"].as_str().unwrap().to_owned();
	(wb, token)
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn transferring_a_shared_draft_does_not_restore_the_former_owner_share(
	#[case] _can_edit: bool,
	#[future(awt)]
	#[with(_can_edit)]
	shared_transfer_workbench: (Workbench, String),
) {
	let (wb, bob) = shared_transfer_workbench;
	let (status, transferred) = request(
		&wb.app,
		&wb.token,
		"POST",
		&format!("{}/transfer", wb.path()),
		json!({"expected_revision":1,"new_owner":"bob"}),
	)
	.await;
	assert_eq!(status, 200, "transfer: {transferred}");
	let (status, shares) = request(
		&wb.app,
		&bob,
		"GET",
		&format!("{}/shares", wb.path()),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200);
	assert_eq!(shares, json!([]));
	let (status, returned) = request(
		&wb.app,
		&bob,
		"POST",
		&format!("{}/transfer", wb.path()),
		json!({"expected_revision":2,"new_owner":"alice"}),
	)
	.await;
	assert_eq!(status, 200, "return transfer: {returned}");
	assert_eq!(
		request(&wb.app, &bob, "GET", &wb.path(), Value::Null)
			.await
			.0,
		403
	);
	assert_eq!(
		request(
			&wb.app,
			&bob,
			"PUT",
			&wb.path(),
			json!({"expected_revision":3,"entry":wb.draft["entry"],"documents":[]})
		)
		.await
		.0,
		403
	);
	wb.cleanup().await;
}

#[rstest::rstest]
#[case("share")]
#[case("transfer")]
#[case("archive")]
#[case("documents")]
#[tokio::test]
async fn draft_authority_changes_wait_for_a_shared_real_dispatch(
	#[future(awt)]
	#[with(true)]
	model_waiting_for_real_tool: (Workbench, Value),
	#[case] change: &str,
) {
	let (wb, started) = model_waiting_for_real_tool;
	wb.effect_gate.paused.store(true, Ordering::SeqCst);
	wb.model_gate.paused.store(false, Ordering::SeqCst);
	wb.model_gate.release.notify_one();
	tokio::time::timeout(Duration::from_secs(5), wb.effect_gate.arrived.notified())
		.await
		.unwrap();
	let (method, path, body) = match change {
		"share" => (
			"POST",
			format!("{}/shares", wb.path()),
			json!({"subject":"bob","can_edit":true,"enabled":false,"include_documents":false}),
		),
		"transfer" => (
			"POST",
			format!("{}/transfer", wb.path()),
			json!({"expected_revision":1,"new_owner":"bob"}),
		),
		"archive" => (
			"POST",
			format!("{}/archive", wb.path()),
			json!({"expected_revision":1,"archived":true}),
		),
		"documents" => (
			"PUT",
			wb.path(),
			json!({"expected_revision":1,"entry":wb.draft["entry"],"documents":[{"name":"new.txt","media_type":"text/plain","text":"Updated references"}]}),
		),
		_ => unreachable!(),
	};
	let app = wb.app.clone();
	let token = wb.f.config.api_token.clone();
	let mut mutation =
		tokio::spawn(async move { request(&app, &token, method, &path, body).await });
	let early = tokio::time::timeout(Duration::from_millis(200), &mut mutation).await;
	wb.effect_gate.paused.store(false, Ordering::SeqCst);
	wb.effect_gate.release.notify_one();
	let finished_early = early.is_ok();
	let result = match early {
		Ok(result) => result.unwrap(),
		Err(_) => mutation.await.unwrap(),
	};
	let finished = wb.operator_finished(&started).await;
	assert!(
		!finished_early,
		"{change} committed across the real dispatch lease: {result:?}"
	);
	assert_eq!(result.0, 200, "mutation: {result:?}");
	assert_eq!(
		wb.effect_hits.load(Ordering::SeqCst),
		1,
		"session: {finished}"
	);
	assert!(
		matches!(finished["status"].as_str(), Some("blocked" | "failed")),
		"session: {finished}"
	);
	wb.cleanup().await;
}

#[rstest::fixture]
async fn installed_tool_waiting_for_dispatch() -> (Workbench, Value, String) {
	let (wb, started) = model_waiting_for_real_tool(false).await;
	let entry = wb.f.registry.get("fixture-tool", "1.0.0").await.unwrap();
	let package =
		wb.f.registry
			.publish(
				&wb.f.store.pool,
				aidash::registry::Package {
					entity: entry,
					author: "Fixture".into(),
					permissions: vec![],
					dependencies: vec![],
				},
			)
			.await
			.unwrap();
	wb.f.registry
		.install(
			&wb.f.store.pool,
			"fixture-tool",
			"1.0.0",
			&package.digest,
			json!({}),
		)
		.await
		.unwrap();
	(wb, started, package.digest)
}

#[rstest::rstest]
#[case("endpoint")]
#[case("replay")]
#[case("credential")]
#[tokio::test]
async fn real_dispatch_rejects_effective_tool_isolation_changes(
	#[future(awt)] installed_tool_waiting_for_dispatch: (Workbench, Value, String),
	#[case] change: &str,
) {
	let (wb, started, digest) = installed_tool_waiting_for_dispatch;
	let overlay = match change {
		"endpoint" => json!({"endpoint":format!("{}/test-effect",wb.endpoint)}),
		"replay" => json!({"replay":"unsafe"}),
		"credential" => json!({"credential_env":"AIDASH_SECRET_TEST_PEER"}),
		_ => unreachable!(),
	};
	wb.f.registry
		.install(&wb.f.store.pool, "fixture-tool", "1.0.0", &digest, overlay)
		.await
		.unwrap();
	wb.model_gate.paused.store(false, Ordering::SeqCst);
	wb.model_gate.release.notify_one();
	let finished = wb.operator_finished(&started).await;
	assert_eq!(
		wb.effect_hits.load(Ordering::SeqCst),
		0,
		"isolation changed: {finished}"
	);
	assert_eq!(finished["status"], "blocked", "session: {finished}");
	assert_eq!(finished["tool_calls"][0]["outcome"], "denied");
	wb.cleanup().await;
}

#[rstest::fixture]
async fn incident_audit_workbench() -> (Workbench, Value) {
	let wb = workbench().await;
	wb.register().await;
	let incident = wb
		.call(
			"POST",
			&format!(
				"/api/workbench/versions/{}/1.0.0/incidents",
				wb.draft["entry"]["id"].as_str().unwrap()
			),
			json!({"severity":"low","owner":"alice","notes":"Initial"}),
		)
		.await;
	assert_eq!(request(&wb.app,&wb.f.config.api_token,"POST","/api/authorization/acme",json!({"expected_revision":1,"bundle":{"tenant":"acme","subjects":{"alice":{"kind":"user"},"bob":{"kind":"user"}},"policies":[{"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},{"id":"hidden-incident","effect":"deny","subjects":{"any":true},"actions":["agent_incident.read"],"resources":{"kinds":["agent_incident"]},"condition":{"op":"eq","left":{"source":"resource","path":"/owner"},"right":{"source":"literal","value":"bob"}}}]}})).await.0,200);
	(wb, incident)
}

#[rstest::rstest]
#[tokio::test]
async fn audit_rechecks_incident_visibility_before_returning_event_history(
	#[future(awt)] incident_audit_workbench: (Workbench, Value),
) {
	let (wb, incident) = incident_audit_workbench;
	let id: uuid::Uuid = incident["id"].as_str().unwrap().parse().unwrap();
	let mut change = wb.f.store.pool.begin().await.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("agent_incidents"))
			.value(Alias::new("owner"), Expr::value("bob"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.execute(&mut *change)
	.await
	.unwrap();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("agent_incident_events"))
			.columns(["incident_id", "actor", "change"].map(Alias::new))
			.values_panic([Expr::cust("$1"), Expr::value("bob"), Expr::cust("$2")])
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(json!({"private":"hidden after owner change"}))
	.execute(&mut *change)
	.await
	.unwrap();
	let app = wb.app.clone();
	let token = wb.token.clone();
	let path = format!(
		"/api/workbench/versions/{}/1.0.0/audit",
		wb.draft["entry"]["id"].as_str().unwrap()
	);
	let mut audit =
		tokio::spawn(async move { request(&app, &token, "GET", &path, Value::Null).await });
	let early = tokio::time::timeout(Duration::from_millis(200), &mut audit).await;
	change.commit().await.unwrap();
	assert!(
		early.is_err(),
		"audit did not hold the incident visibility boundary: {early:?}"
	);
	let (status, page) = audit.await.unwrap();
	assert_eq!(status, 200, "audit: {page}");
	assert!(
		page["items"]
			.as_array()
			.unwrap()
			.iter()
			.all(|item| item["source"] != "incident"),
		"audit: {page}"
	);
	wb.cleanup().await;
}

#[rstest::fixture]
async fn registered_document_workbench(#[default("replace")] change: &str) -> Workbench {
	let mut wb = workbench().await;
	let documents = if change == "add" {
		json!([])
	} else {
		json!([{"name":"notes.txt","media_type":"text/plain","text":"Original references"}])
	};
	wb.draft = wb
		.call(
			"PUT",
			&wb.path(),
			json!({"expected_revision":1,"entry":wb.draft["entry"],"documents":documents}),
		)
		.await;
	wb.call(
		"POST",
		&format!("{}/register", wb.path()),
		json!({"expected_revision":2}),
	)
	.await;
	let documents = if change == "remove" {
		json!([])
	} else if change == "unchanged" {
		documents
	} else {
		json!([{"name":"notes.txt","media_type":"text/plain","text":"Updated references"}])
	};
	wb.draft = wb
		.call(
			"PUT",
			&wb.path(),
			json!({"expected_revision":2,"entry":wb.draft["entry"],"documents":documents}),
		)
		.await;
	wb
}

#[rstest::rstest]
#[case("add")]
#[case("replace")]
#[case("remove")]
#[case("unchanged")]
#[tokio::test]
async fn version_history_compares_registered_knowledge_to_current_saved_documents(
	#[case] change: &str,
	#[future(awt)]
	#[with(change)]
	registered_document_workbench: Workbench,
) {
	use sha2::{Digest, Sha256};
	let wb = registered_document_workbench;
	let versions = wb
		.call("GET", &format!("{}/versions", wb.path()), Value::Null)
		.await;
	let expected = if change == "remove" {
		Value::Null
	} else {
		json!(format!(
			"{:x}",
			Sha256::digest(wb.draft["documents"].to_string().as_bytes())
		))
	};
	assert_eq!(versions[0]["draft_knowledge_digest"], expected);
	assert_eq!(
		versions[0]["entry"]["config"]["knowledge_digest"] == expected,
		change == "unchanged"
	);
	wb.cleanup().await;
}

#[rstest::fixture]
async fn review6_profile_backlog() -> Workbench {
	let wb = workbench().await;
	let result = request(&wb.app,&wb.f.config.api_token,"PUT","/api/workbench/test-profiles/acme/zz-compatible",json!({"expected_revision":0,"enabled":true,"rules":[{"tool":{"id":"fixture-tool","version":"1.0.0"},"endpoint":format!("{}/test-effect",wb.endpoint),"credential_env":null,"allowed_actions":["read"],"allowed_resources":["sandbox"]}]})).await;
	assert_eq!(result.0, 200, "profile: {result:?}");
	for index in 0..105 {
		sqlx::query(&Query::insert().into_table(Alias::new("agent_test_profiles")).columns(["tenant","id","revision","enabled","rules"].map(Alias::new)).values_panic([Expr::value("acme"),Expr::cust("$1"),Expr::value(1),Expr::value(true),Expr::cust("$2")]).to_string(PostgresQueryBuilder))
		.bind(format!("aa-{index:03}"))
		.bind(json!([{"tool":{"id":"other-tool","version":"1.0.0"},"endpoint":"http://127.0.0.1:9","credential_env":null,"allowed_actions":["read"],"allowed_resources":["sandbox"]}]))
		.execute(&wb.f.store.pool).await.unwrap();
	}
	wb
}

#[rstest::rstest]
#[tokio::test]
async fn review6_compatible_profiles_survive_an_incompatible_backlog(
	#[future(awt)] review6_profile_backlog: Workbench,
) {
	let wb = review6_profile_backlog;
	let profiles = wb
		.call(
			"GET",
			&format!(
				"/api/workbench/test-profiles?tenant=acme&draft_id={}",
				wb.draft["id"].as_str().unwrap()
			),
			Value::Null,
		)
		.await;
	assert_eq!(
		profiles.as_array().unwrap().len(),
		1,
		"profiles: {profiles}"
	);
	assert_eq!(profiles[0]["id"], "zz-compatible");
	wb.cleanup().await;
}

#[rstest::fixture]
async fn review6_revoked_inspection() -> (Workbench, axum::Router) {
	let wb = workbench().await;
	wb.register().await;
	let authorization = aidash::authorization::Authorization {
		pool: wb.f.store.pool.clone(),
	};
	let actor = authorization.authenticate(&wb.token).await.unwrap();
	let credential = authorization.credentials("acme").await.unwrap().remove(0);
	authorization
		.revoke_credential("acme", credential.id)
		.await
		.unwrap();
	let (routes, _) = aidash::workbench::routes().split_for_parts();
	let app = axum::Router::new()
		.nest("/api", routes)
		.layer(axum::Extension(actor))
		.with_state(wb.f.clone());
	(wb, app)
}

#[rstest::rstest]
#[tokio::test]
async fn review6_permission_context_rejects_a_previously_authenticated_revoked_actor(
	#[future(awt)] review6_revoked_inspection: (Workbench, axum::Router),
) {
	let (wb, app) = review6_revoked_inspection;
	let (status, body) = request(
		&app,
		&wb.token,
		"POST",
		&format!(
			"/api/workbench/versions/{}/1.0.0/permissions",
			wb.draft["entry"]["id"].as_str().unwrap()
		),
		json!({"tenant":"acme","subject":"alice"}),
	)
	.await;
	assert_eq!(status, 401, "context: {body}");
	wb.cleanup().await;
}

#[rstest::fixture]
async fn review6_delegation_context(#[default(false)] delegation: bool) -> Workbench {
	let mut wb = workbench().await;
	let mut child = wb.draft["entry"].clone();
	child["id"] = json!("delegated-agent");
	child["config"]["tools"] = json!([]);
	assert_eq!(
		request(
			&wb.app,
			&wb.f.config.api_token,
			"POST",
			"/api/registry",
			child
		)
		.await
		.0,
		200
	);
	assert_eq!(request(&wb.app,&wb.f.config.api_token,"POST","/api/registry",json!({"id":"agent-tool","version":"1.0.0","kind":"tool","name":{"en":"Agent Tool"},"description":{"en":"Delegate"},"config":{"transport":"agent","node_id":wb.f.config.node_id,"agent":{"id":"delegated-agent","version":"1.0.0"}}})).await.0,200);
	let mut entry = wb.draft["entry"].clone();
	entry["config"]["tools"] = json!([{"id":"agent-tool","version":"1.0.0"}]);
	entry["config"]["allow_task_delegation"] = json!(delegation);
	wb.draft = wb
		.call(
			"PUT",
			&wb.path(),
			json!({"expected_revision":1,"entry":entry}),
		)
		.await;
	wb.call(
		"POST",
		&format!("{}/register", wb.path()),
		json!({"expected_revision":2}),
	)
	.await;
	wb
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn review6_permission_context_respects_agent_tool_behavior_flags(
	#[case] delegation: bool,
	#[future(awt)]
	#[with(delegation)]
	review6_delegation_context: Workbench,
) {
	let wb = review6_delegation_context;
	let context = wb
		.call(
			"POST",
			&format!(
				"/api/workbench/versions/{}/1.0.0/permissions",
				wb.draft["entry"]["id"].as_str().unwrap()
			),
			json!({"tenant":"acme","subject":"alice"}),
		)
		.await;
	assert_eq!(
		context["rows"]
			.as_array()
			.unwrap()
			.iter()
			.any(|row| row["reference"]["id"] == "agent-tool"),
		delegation,
		"context: {context}"
	);
	wb.cleanup().await;
}

#[rstest::fixture]
async fn review6_expired_retention() -> (Workbench, Value) {
	let wb = workbench().await;
	wb.register().await;
	let incident = wb
		.call(
			"POST",
			&format!(
				"/api/workbench/versions/{}/1.0.0/incidents",
				wb.draft["entry"]["id"].as_str().unwrap()
			),
			json!({"severity":"low","owner":"alice","notes":"Expired"}),
		)
		.await;
	sqlx::query(
		&Query::update()
			.table(Alias::new("agent_incidents"))
			.value(Alias::new("status"), Expr::value("resolved"))
			.value(Alias::new("evidence_expires_at"), Expr::cust("$2"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(
		incident["id"]
			.as_str()
			.unwrap()
			.parse::<uuid::Uuid>()
			.unwrap(),
	)
	.bind(chrono::Utc::now() - chrono::Duration::hours(1))
	.execute(&wb.f.store.pool)
	.await
	.unwrap();
	(wb, incident)
}

#[rstest::rstest]
#[case("resolved")]
#[case("open")]
#[tokio::test]
async fn review6_expired_retention_rejects_new_evidence_before_cleanup(
	#[future(awt)] review6_expired_retention: (Workbench, Value),
	#[case] status: &str,
) {
	let (wb, incident) = review6_expired_retention;
	let (code,body) = request(&wb.app,&wb.token,"PUT",&format!("/api/workbench/incidents/{}",incident["id"].as_str().unwrap()),json!({"expected_revision":1,"severity":"low","status":status,"owner":"alice","notes":"Must not revive","add_evidence":[{"title":"Late","content":"new secret"}]})).await;
	assert_eq!(code, 409, "incident: {body}");
	wb.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn review6_operator_transfer_holds_target_eligibility_through_commit(
	#[future(awt)] workbench: Workbench,
) {
	let wb = workbench;
	let mut barrier = wb.f.store.pool.begin().await.unwrap();
	sqlx::query(
		&Query::select()
			.expr(Expr::cust("PG_ADVISORY_XACT_LOCK(71003201)"))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *barrier)
	.await
	.unwrap();
	let app = wb.app.clone();
	let token = wb.f.config.api_token.clone();
	let path = format!("{}/transfer", wb.path());
	let transfer = tokio::spawn(async move {
		request(
			&app,
			&token,
			"POST",
			&path,
			json!({"expected_revision":1,"new_owner":"bob"}),
		)
		.await
	});
	tokio::time::timeout(Duration::from_secs(5), async {
		loop {
			let waiting: bool = sqlx::query_scalar(
				&Query::select()
					.expr(Expr::cust(
						"EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name=$1 AND wait_event='advisory')",
					))
					.to_string(PostgresQueryBuilder),
			)
			.bind(&wb.schema)
			.fetch_one(&wb.f.store.pool)
			.await
			.unwrap();
			if waiting {
				break;
			}
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
	})
	.await
	.expect("transfer did not reach its commit boundary");
	let app = wb.app.clone();
	let token = wb.f.config.api_token.clone();
	let mut disable = tokio::spawn(async move {
		request(&app,&token,"POST","/api/authorization/acme",json!({"expected_revision":1,"bundle":{"tenant":"acme","subjects":{"alice":{"kind":"user"},"bob":{"kind":"user","enabled":false}},"policies":[{"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]}})).await
	});
	let early = tokio::time::timeout(Duration::from_millis(200), &mut disable).await;
	barrier.commit().await.unwrap();
	let finished_early = early.is_ok();
	let disabled = match early {
		Ok(result) => result.unwrap(),
		Err(_) => disable.await.unwrap(),
	};
	let transferred = transfer.await.unwrap();
	assert!(
		!finished_early,
		"target disable crossed the transfer transaction: {disabled:?}"
	);
	assert_eq!(
		(transferred.0, disabled.0),
		(200, 200),
		"transfer: {transferred:?}; disable: {disabled:?}"
	);
	wb.cleanup().await;
}
