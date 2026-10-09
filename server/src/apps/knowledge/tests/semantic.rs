use common::upstream_fixtures as upstream;
use reinhardt::ServerRouter as Router;
use rstest::fixture;
use upstream::reply;
#[path = "../../execution/tests/support/legacy.rs"]
mod common;
use aidash_server::semantic::{self, ConfigureIndex, EmbeddingConfig, IndexSpec, VectorConfig};

use common::request;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

struct ControlledContext {
	_fixture: common::ApplicationFixture,
	f: aidash_server::federation::Federation,
	worker: aidash_server::harness::Harness,
	url: String,
	schema: String,
	captured: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
	model_server: reinhardt::test::fixtures::server::TestServerGuard,
	embedding_server: std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard>,
}

#[rstest::fixture]
async fn controlled_context(
	#[default(true)] memory: bool,
	#[default(true)] workspace_retrieval: bool,
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,

	#[future(awt)]
	#[from(controlled_context_provider)]
	fixture: ControlledContextProvider,

	#[future(awt)] embeddings: std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) -> ControlledContext {
	let captured = fixture.state.captured;
	let model_server = fixture.server;

	use aidash_server::domain::{ArtifactInput, qualified_agent};
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();

	let endpoint = model_server.url.clone();
	let (mut policy, token, task) = common::bootstrap_with_context(&f, &app, &endpoint, true).await;
	let mut agent = f.registry.get("research", "1.0.0").await.unwrap();
	agent.version = "1.0.1".into();
	agent.binding_normalization = None;
	let mut bindings = vec![];
	for (enabled, id, kind, adapter) in [
		(memory, "context-memory", "memory", "semantic_memory"),
		(
			workspace_retrieval,
			"context-workspace",
			"source",
			"workspace_retrieval",
		),
	] {
		if !enabled {
			continue;
		}
		let entry = json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"Explicit native context"},"config":{"schema_version":1,"source":{"adapter":adapter}}});
		f.registry
			.register(serde_json::from_value(entry).unwrap())
			.await
			.unwrap();
		assert_eq!(
			request(
				&app,
				&f.config.api_token,
				"POST",
				"/api/authorization/acme/catalog",
				json!({"entry":{"id":id,"version":"1.0.0"},"expected_revision":0,"enabled":true})
			)
			.await
			.0,
			200
		);
		bindings.push(json!({"kind":kind,"target":{"registry_node":f.config.node_id,"id":id,"version":"1.0.0"},"narrow":{}}));
	}
	agent.config["bindings"] = json!(bindings);
	f.registry.register(agent).await.unwrap();
	let owner = qualified_agent(&f.config.node_id, "research", "1.0.1");
	policy["subjects"][&owner] = json!({"kind":"agent"});
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"research","version":"1.0.1"},"expected_revision":0,"enabled":true})
		)
		.await
		.0,
		200
	);
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	let embedding_endpoint = embeddings.url.clone();
	let embedding_server = embeddings;
	configure(
		&app,
		&f.config.api_token,
		workspace,
		&spec(&embedding_endpoint),
		0,
	)
	.await;
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/tasks/{task}/claim"),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.1"}})
		)
		.await
		.0,
		200
	);
	let worker = aidash_server::harness::Harness {
		federation: f.for_workers().await.unwrap(),
	};
	worker.worker_once().await.unwrap();
	let artifact = f
		.store
		.publish_artifact(
			task,
			&owner,
			"controlled-artifact",
			&ArtifactInput {
				kind: "text".into(),
				name: "Car report".into(),
				content: json!("A car has a steering wheel."),
			},
		)
		.await
		.unwrap();
	let message = f
		.store
		.message_record(
			workspace,
			"human",
			"Vehicle safety",
			Some("controlled-message"),
		)
		.await
		.unwrap();
	for (key, source) in [
		(
			"memory",
			json!({"kind":"memory","text":"A car carries passengers."}),
		),
		("artifact", json!({"kind":"artifact","id":artifact.id})),
		("message", json!({"kind":"message","id":message.id})),
	] {
		let (status, entry) = request(
			&app,
			&token,
			"POST",
			&format!("/api/workspaces/{workspace}/semantic/entries"),
			json!({"key":key,"expected_revision":0,"source":source,"metadata":{}}),
		)
		.await;
		assert_eq!(status, 200, "source: {entry}");
	}
	semantic::worker::sweep(&f.store).await.unwrap();
	ControlledContext {
		_fixture: application_fixture,
		f,
		worker,
		url,
		schema,
		captured,
		model_server,
		embedding_server,
	}
}

#[rstest::rstest]
#[case(false, false)]
#[case(false, true)]
#[case(true, false)]
#[case(true, true)]
#[tokio::test]
async fn automatic_semantic_context_respects_each_agent_behavior_control(
	#[case] memory: bool,
	#[case] workspace_retrieval: bool,
	#[future(awt)]
	#[with(memory, workspace_retrieval)]
	controlled_context: ControlledContext,
) {
	let fixture = controlled_context;
	fixture.worker.worker_once().await.unwrap();
	let contexts = fixture.captured.lock().unwrap().clone();
	assert_eq!(
		contexts.len(),
		1,
		"run: {:?}",
		fixture.f.store.runs().await.unwrap()[0]
	);
	let semantic = &contexts[0]["current"]["semantic_memory"]["workspace"];
	if !memory && !workspace_retrieval {
		assert!(semantic.is_null(), "disabled retrieval: {semantic}");
	} else {
		let mut kinds: Vec<_> = semantic["matches"]
			.as_array()
			.unwrap()
			.iter()
			.map(|m| m["source"]["kind"].as_str().unwrap())
			.collect();
		kinds.sort();
		let mut expected = Vec::new();
		if memory {
			expected.push("memory");
		}
		if workspace_retrieval {
			expected.extend(["artifact", "message"]);
		}
		expected.sort();
		assert_eq!(kinds, expected, "semantic: {semantic}");
	}
	fixture.worker.federation.store.pool.close().await;
	fixture.worker.federation.store.control_pool.close().await;
	drop(fixture.model_server);
	drop(fixture.embedding_server);
	dispose(fixture.f, &fixture.url, &fixture.schema).await;
}

fn spec(endpoint: &str) -> IndexSpec {
	IndexSpec {
		embedding: EmbeddingConfig {
			provider: "openai".into(),
			endpoint: format!("{endpoint}/v1"),
			credential_env: None,
			model: "fixture".into(),
			model_version: "1".into(),
			dimensions: 3,
		},
		vector: VectorConfig {
			provider: "postgres".into(),
			endpoint: "local".into(),
			credential_env: None,
		},
		enabled: true,
		auto_context: true,
		max_sources: 64,
		max_results: 10,
		max_result_tokens: 4096,
		max_input_bytes: 8192,
	}
}
async fn configure(
	app: &common::TestApplication,
	operator: &str,
	workspace: Uuid,
	spec: &IndexSpec,
	revision: i64,
) -> Value {
	let (status, index) = request(
		app,
		operator,
		"POST",
		&format!("/api/workspaces/{workspace}/semantic/index"),
		serde_json::to_value(ConfigureIndex {
			expected_revision: revision,
			spec: spec.clone(),
		})
		.unwrap(),
	)
	.await;
	assert_eq!(status, 200, "{index}");
	index
}
async fn put(
	app: &common::TestApplication,
	token: &str,
	workspace: Uuid,
	key: &str,
	text: &str,
	revision: i64,
) -> Value {
	let (status,entry)=request(app,token,"POST",&format!("/api/workspaces/{workspace}/semantic/entries"),json!({"key":key,"source":{"kind":"memory","text":text},"expected_revision":revision,"metadata":{"topic":"public"}})).await;
	assert_eq!(status, 200, "{entry}");
	entry
}
async fn search(app: &common::TestApplication, token: &str, workspace: Uuid) -> (u16, Value) {
	request(
		app,
		token,
		"POST",
		&format!("/api/workspaces/{workspace}/semantic/search"),
		json!({"query":"vehicle","limit":1,"max_tokens":2048}),
	)
	.await
}
async fn dispose(f: aidash_server::federation::Federation, url: &str, schema: &str) {
	let rows: Vec<(String, Value)> = sqlx::query_as(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::SimpleExpr::from(
				reinhardt::query::Expr::col(reinhardt::query::Alias::new("collection")),
			))
			.expr(reinhardt::query::SimpleExpr::from(
				reinhardt::query::Expr::col(reinhardt::query::Alias::new("vector")),
			))
			.from(reinhardt::query::Alias::new("semantic_collections"))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_all(f.store.pool.driver())
	.await
	.unwrap();
	for (collection, config) in rows {
		semantic::backend::delete_collection(
			&f.store,
			&serde_json::from_value(config).unwrap(),
			&collection,
		)
		.await
		.unwrap();
	}
	common::cleanup(f, url, schema).await;
}

#[rstest::rstest]
#[case::openai("openai")]
#[case::openrouter("openrouter")]
#[tokio::test]
async fn semantic_lifecycle_is_durable_revisioned_and_not_keyword_search(
	#[case] provider: &str,
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,

	#[future(awt)] embeddings: std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) {
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let endpoint = embeddings.url.clone();
	let server = embeddings;
	let (_, token, task) = common::bootstrap(&f, &app, &endpoint).await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	let mut config = spec(&endpoint);
	config.embedding.provider = provider.into();
	let first = configure(&app, &f.config.api_token, workspace, &config, 0).await;
	assert_eq!(
		configure(&app, &f.config.api_token, workspace, &config, 0).await["revision"],
		1
	);
	let cars = put(
		&app,
		&token,
		workspace,
		"cars",
		"A car carries passengers.",
		0,
	)
	.await;
	assert_eq!(
		put(
			&app,
			&token,
			workspace,
			"cars",
			"A car carries passengers.",
			0
		)
		.await["id"],
		cars["id"]
	);
	put(
		&app,
		&token,
		workspace,
		"bread",
		"Bread is baked in an oven.",
		0,
	)
	.await;
	assert_eq!(
		search(&app, &token, workspace).await.0,
		409,
		"pending data is never a successful empty search"
	);
	semantic::worker::sweep(&f.store).await.unwrap();
	let (status, result) = search(&app, &token, workspace).await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(result["matches"][0]["entry_id"], cars["id"]);
	assert_eq!(result["matches"][0]["source"], json!({"kind":"memory"}));
	assert!(
		!result["matches"][0]["text"]
			.as_str()
			.unwrap()
			.contains("vehicle")
	);
	assert_eq!(result["model_version"], "1");
	// Replace the source using an optimistic revision. The old point must not
	// remain eligible even before the vector deletion has run.
	let updated = put(
		&app,
		&token,
		workspace,
		"cars",
		"A car uses roads safely.",
		1,
	)
	.await;
	assert_eq!(updated["revision"], 2);
	assert_eq!(search(&app, &token, workspace).await.0, 409);
	semantic::worker::sweep(&f.store).await.unwrap();
	assert_eq!(
		search(&app, &token, workspace).await.1["matches"][0]["revision"],
		2
	);
	config.embedding.model_version = "2".into();
	let second = configure(&app, &f.config.api_token, workspace, &config, 1).await;
	assert_ne!(first["collection"], second["collection"]);
	assert_eq!(search(&app, &token, workspace).await.0, 409);
	// Fresh pools reconstruct authority and jobs entirely from durable state.
	let restarted = f.for_workers().await.unwrap();
	semantic::worker::sweep(&restarted.store).await.unwrap();
	assert_eq!(
		search(&app, &token, workspace).await.1["model_version"],
		"2"
	);
	restarted.store.pool.close().await;
	restarted.store.control_pool.close().await;
	let current_points = request(
		&app,
		&token,
		"GET",
		&format!("/api/workspaces/{workspace}/semantic/entries"),
		json!(null),
	)
	.await
	.1;
	let current_point = current_points
		.as_array()
		.unwrap()
		.iter()
		.find(|e| e["id"] == cars["id"])
		.unwrap()["point_id"]
		.clone();
	let path = format!(
		"/api/workspaces/{workspace}/semantic/entries/{}",
		cars["id"].as_str().unwrap()
	);
	assert_eq!(
		request(
			&app,
			&token,
			"DELETE",
			&path,
			json!({"expected_revision":2})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(
			&app,
			&token,
			"DELETE",
			&path,
			json!({"expected_revision":2})
		)
		.await
		.0,
		200
	);
	let after = search(&app, &token, workspace).await;
	assert_eq!(after.0, 200, "{}", after.1);
	assert_ne!(after.1["matches"][0]["entry_id"], cars["id"]);
	semantic::worker::sweep(&f.store).await.unwrap();
	use aidash_application::ports::VectorIndex;
	assert!(
		!aidash_server::bootstrap::semantic_transport(&f.store)
			.present(
				&config.vector,
				second["collection"].as_str().unwrap(),
				&[serde_json::from_value(current_point.clone()).unwrap()]
			)
			.await
			.unwrap()
	);
	drop(server);
	dispose(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_access_is_checked_before_search_and_jobs_retain_revocation(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,

	#[future(awt)] embeddings: std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) {
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let endpoint = embeddings.url.clone();
	let server = embeddings;
	let (mut policy, token, task) = common::bootstrap(&f, &app, &endpoint).await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	configure(&app, &f.config.api_token, workspace, &spec(&endpoint), 0).await;
	let cars = put(
		&app,
		&token,
		workspace,
		"cars",
		"A car carries passengers.",
		0,
	)
	.await;
	semantic::worker::sweep(&f.store).await.unwrap();
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"revoke-semantic","effect":"deny","subjects":{"ids":["alice"]},"actions":["semantic.read"],"resources":{"kinds":["semantic"]}}));
	let (status, response) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	let reindex_path = format!(
		"/api/workspaces/{workspace}/semantic/entries/{}/reindex",
		cars["id"].as_str().unwrap()
	);
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&reindex_path,
			json!({"expected_revision":1})
		)
		.await
		.0,
		403,
		"write-only permission cannot disclose a stored source in mutation responses"
	);
	// No allowed candidates means no embedding request is necessary, even when
	// the embedding backend is down. The caller cannot infer denied sources.
	drop(server);
	let response = search(&app, &token, workspace).await;
	assert_eq!(response.0, 200, "{}", response.1);
	assert_eq!(response.1["matches"], json!([]));
	let (status, credentials) = request(
		&app,
		&f.config.api_token,
		"GET",
		"/api/authorization/acme/credentials",
		json!(null),
	)
	.await;
	assert_eq!(status, 200, "{credentials}");
	let credential = credentials
		.as_array()
		.unwrap()
		.iter()
		.find(|c| c["subject"] == "alice")
		.unwrap();
	let auth = aidash_server::authorization::Authorization {
		pool: f.store.pool.clone(),
	};
	auth.revoke_credential(
		"acme",
		Uuid::parse_str(credential["id"].as_str().unwrap()).unwrap(),
	)
	.await
	.unwrap();
	{
		let query_bind_1 = Uuid::parse_str(cars["id"].as_str().unwrap()).unwrap();
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("semantic_entries"))
				.value_expr(
					reinhardt::query::Alias::new("next_attempt"),
					reinhardt::query::Expr::cust("CLOCK_TIMESTAMP()"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
	}
	.unwrap();
	semantic::worker::sweep(&f.store).await.unwrap();
	let state: String = {
		let query_bind_1 = Uuid::parse_str(cars["id"].as_str().unwrap()).unwrap();
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("state")),
				))
				.from(reinhardt::query::Alias::new("semantic_entries"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(state, "REVOKED");
	assert_eq!(search(&app, &token, workspace).await.0, 401);
	dispose(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_outage_and_input_bounds_are_visible_and_retriable(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,

	#[future(awt)] embeddings: std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) {
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let endpoint = embeddings.url.clone();
	let server = embeddings;
	let (_, token, task) = common::bootstrap(&f, &app, &endpoint).await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	let mut config = spec(&endpoint);
	config.embedding.dimensions = 4; // Provider's actual width is three.
	configure(&app, &f.config.api_token, workspace, &config, 0).await;
	let cars = put(
		&app,
		&token,
		workspace,
		"cars",
		"A car carries passengers.",
		0,
	)
	.await;
	semantic::worker::sweep(&f.store).await.unwrap();
	let entries = request(
		&app,
		&token,
		"GET",
		&format!("/api/workspaces/{workspace}/semantic/entries"),
		json!(null),
	)
	.await;
	assert_eq!(entries.0, 200);
	assert_eq!(entries.1[0]["state"], "ERROR");
	assert_eq!(entries.1[0]["attempts"], 1);
	assert_eq!(search(&app, &token, workspace).await.0, 409);
	config.embedding.dimensions = 3;
	configure(&app, &f.config.api_token, workspace, &config, 1).await;
	semantic::worker::sweep(&f.store).await.unwrap();
	assert_eq!(search(&app, &token, workspace).await.0, 200);
	let reindex_path = format!(
		"/api/workspaces/{workspace}/semantic/entries/{}/reindex",
		cars["id"].as_str().unwrap()
	);
	let first_reindex = request(
		&app,
		&token,
		"POST",
		&reindex_path,
		json!({"expected_revision":1}),
	)
	.await;
	let replay_reindex = request(
		&app,
		&token,
		"POST",
		&reindex_path,
		json!({"expected_revision":1}),
	)
	.await;
	assert_eq!(first_reindex.0, 200);
	assert_eq!(replay_reindex.0, 200);
	assert_eq!(first_reindex.1["point_id"], replay_reindex.1["point_id"]);
	semantic::worker::sweep(&f.store).await.unwrap();
	let (status, _) = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{workspace}/semantic/search"),
		json!({"query":"vehicle","limit":999,"max_tokens":2048}),
	)
	.await;
	assert_eq!(status, 400);
	let (status,_)=request(&app,&token,"POST",&format!("/api/workspaces/{workspace}/semantic/entries"),json!({"key":"cars","expected_revision":99,"source":{"kind":"memory","text":"changed"},"metadata":{}})).await;
	assert_eq!(status, 409);
	drop(server);
	let response = search(&app, &token, workspace).await;
	assert_eq!(response.0, 503, "embedding outage must fail visibly");
	assert!(response.1["matches"].is_null());
	assert!(!response.1.to_string().contains(&endpoint));
	assert!(!cars["id"].is_null());
	dispose(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_context_is_provenanced_and_revocation_hides_run_journals(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,

	#[future(awt)]
	#[from(semantic_context_is_provenanced_and_revocation_hides_run_journals_provider)]
	fixture: SemanticContextIsProvenancedAndRevocationHidesRunJournalsProvider,

	#[future(awt)] embeddings: std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) {
	let captured = fixture.state.captured;
	let server = fixture.server;

	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let embedding_endpoint = embeddings.url.clone();
	let embedding_server = embeddings;

	let endpoint = server.url.clone();
	let (mut policy, token, task) = common::bootstrap_with_context(&f, &app, &endpoint, true).await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	configure(
		&app,
		&f.config.api_token,
		workspace,
		&spec(&embedding_endpoint),
		0,
	)
	.await;
	let cars = put(
		&app,
		&token,
		workspace,
		"cars",
		"A car carries passengers.",
		0,
	)
	.await;
	semantic::worker::sweep(&f.store).await.unwrap();
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/tasks/{task}/claim"),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}})
		)
		.await
		.0,
		200
	);
	let worker = aidash_server::harness::Harness {
		federation: f.for_workers().await.unwrap(),
	};
	worker.worker_once().await.unwrap();
	worker.worker_once().await.unwrap();
	let run = f.store.runs().await.unwrap().remove(0);
	assert_eq!(run.phase().as_str(), "TOOL_CALL", "{:?}", run.error);
	let context = captured.lock().unwrap()[0].clone();
	assert_eq!(
		context["current"]["semantic_memory"]["workspace"]["matches"][0]["entry_id"],
		cars["id"]
	);
	assert_eq!(
		context["current"]["semantic_memory"]["workspace"]["model_version"],
		"1"
	);
	let count: i64 = {
		let query_bind_1 = run.id;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust("COUNT(*)"))
				.from(reinhardt::query::Alias::new("semantic_run_reads"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(run_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(count, 1);
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-inline-semantic-memory","effect":"deny","subjects":{"ids":["alice"]},"actions":["semantic.read"],"resources":{"kinds":["semantic"]}}));
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy})
		)
		.await
		.0,
		200
	);
	let visible = request(
		&app,
		&token,
		"GET",
		&format!("/api/workspaces/{workspace}/semantic/entries"),
		json!(null),
	)
	.await
	.1;
	assert_eq!(
		visible.as_array().unwrap().len(),
		0,
		"inline source reads require the semantic entry permission"
	);
	policy["policies"].as_array_mut().unwrap().pop();
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":2,"bundle":policy})
		)
		.await
		.0,
		200
	);
	let path = format!(
		"/api/workspaces/{workspace}/semantic/entries/{}",
		cars["id"].as_str().unwrap()
	);
	assert_eq!(
		request(
			&app,
			&token,
			"DELETE",
			&path,
			json!({"expected_revision":1})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(
			&app,
			&token,
			"GET",
			&format!("/api/runs/{}", run.id),
			json!(null)
		)
		.await
		.0,
		403
	);
	worker.worker_once().await.unwrap();
	assert_eq!(
		f.store.run(run.id).await.unwrap().control.as_str(),
		"PAUSED"
	);
	assert_eq!(captured.lock().unwrap().len(), 1);
	worker.federation.store.pool.close().await;
	worker.federation.store.control_pool.close().await;
	drop(server);
	drop(embedding_server);
	dispose(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn linked_sources_and_agent_metadata_filters_respect_original_authority(
	#[future(awt)]
	#[from(common::native_application)]
	application_fixture: common::ApplicationFixture,

	#[future(awt)] embeddings: std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) {
	use aidash_server::domain::{ArtifactInput, qualified_agent};
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let endpoint = embeddings.url.clone();
	let server = embeddings;
	let (mut policy, token, task) = common::bootstrap(&f, &app, &endpoint).await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	configure(&app, &f.config.api_token, workspace, &spec(&endpoint), 0).await;
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/tasks/{task}/claim"),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}})
		)
		.await
		.0,
		200
	);
	aidash_server::harness::Harness {
		federation: f.clone(),
	}
	.worker_once()
	.await
	.unwrap();
	let artifact = f
		.store
		.publish_artifact(
			task,
			&qualified_agent(&f.config.node_id, "research", "1.0.0"),
			"semantic-artifact",
			&ArtifactInput {
				kind: "text".into(),
				name: "Car report".into(),
				content: json!("A car has a steering wheel."),
			},
		)
		.await
		.unwrap();
	f.store
		.message(
			workspace,
			"human",
			"Vehicle safety",
			Some("semantic-vehicle-safety"),
		)
		.await
		.unwrap();
	let message = f.store.snapshot(workspace).await.unwrap().messages[0].id;
	let mut ids = vec![];
	for (kind, id, agent) in [
		("artifact", artifact.id, None),
		("message", message, Some("aidash://private/agent@1.0.0")),
	] {
		let (status,entry)=request(&app,&token,"POST",&format!("/api/workspaces/{workspace}/semantic/entries"),json!({"key":kind,"expected_revision":0,"source":{"kind":kind,"id":id},"agent":agent,"metadata":{"topic":"transport"}})).await;
		assert_eq!(status, 200, "{entry}");
		ids.push(entry);
	}
	semantic::worker::sweep(&f.store).await.unwrap();
	let found = search(&app, &token, workspace).await;
	assert_eq!(found.0, 200, "{}", found.1);
	assert_eq!(
		found.1["matches"][0]["entry_id"], ids[0]["id"],
		"agent-scoped data is not implicitly included"
	);
	let path = format!("/api/workspaces/{workspace}/semantic/search");
	let (_,found)=request(&app,&token,"POST",&path,json!({"query":"vehicle","agent":"aidash://private/agent@1.0.0","metadata":{"topic":"transport"},"limit":10,"max_tokens":4096})).await;
	assert_eq!(found["matches"].as_array().unwrap().len(), 2);
	let (_, filtered) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"query":"vehicle","metadata":{"topic":"cooking"},"limit":10,"max_tokens":4096}),
	)
	.await;
	assert_eq!(filtered["matches"], json!([]));
	// An out-of-band source update invalidates a READY vector immediately. A
	// background sweep allocates a new revision and then indexes the new text.
	{
		let query_bind_1 = artifact.id;
		let query_bind_2 = json!("A car needs brakes.");
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("artifacts"))
				.value_expr(
					reinhardt::query::Alias::new("content"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(search(&app, &token, workspace).await.0, 409);
	sqlx::query(
		&reinhardt::query::Query::update()
			.table(reinhardt::query::Alias::new("semantic_entries"))
			.value_expr(
				reinhardt::query::Alias::new("next_attempt"),
				reinhardt::query::Expr::cust("CLOCK_TIMESTAMP()"),
			)
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.execute(f.store.pool.driver())
	.await
	.unwrap();
	semantic::worker::sweep(&f.store).await.unwrap();
	semantic::worker::sweep(&f.store).await.unwrap();
	assert!(
		search(&app, &token, workspace).await.1["matches"][0]["text"]
			.as_str()
			.unwrap()
			.contains("brakes")
	);
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-original-artifact","effect":"deny","subjects":{"ids":["alice"]},"actions":["artifact.read"],"resources":{"kinds":["artifact"],"ids":[artifact.id.to_string()]}}));
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		search(&app, &token, workspace).await.1["matches"],
		json!([])
	);
	let (_, entries) = request(
		&app,
		&token,
		"GET",
		&format!("/api/workspaces/{workspace}/semantic/entries"),
		json!(null),
	)
	.await;
	assert!(!entries.to_string().contains(&artifact.id.to_string()));
	// Even an operator cannot attach a resource from a different workspace.
	let other = f
		.store
		.create_workspace("Other", "Isolation")
		.await
		.unwrap();
	configure(&app, &f.config.api_token, other.id, &spec(&endpoint), 0).await;
	assert_eq!(request(&app,&f.config.api_token,"POST",&format!("/api/workspaces/{}/semantic/entries",other.id),json!({"key":"forged","expected_revision":0,"source":{"kind":"artifact","id":artifact.id},"metadata":{}})).await.0,403);
	assert_eq!(search(&app, &token, other.id).await.0, 403);
	drop(server);
	dispose(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_postgres_restart_and_lost_points_recover(
	#[from(common::isolated_test_environment)] environment: futures_util::future::Shared<
		futures_util::future::BoxFuture<'static, Arc<common::TestEnvironment>>,
	>,
	#[future(awt)]
	#[from(isolated_application)]
	#[with(environment.clone())]
	application_fixture: common::ApplicationFixture,
	#[future(awt)] embeddings: std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard>,
) {
	use aidash_application::ports::VectorIndex;
	let (f, url, schema) = application_fixture.runtime.parts();
	let app = application_fixture.application.clone();
	let embedding = embeddings.url.clone();
	let server = embeddings;
	let (_, token, task) = common::bootstrap(&f, &app, &embedding).await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	let config = spec(&embedding);
	let index = configure(&app, &f.config.api_token, workspace, &config, 0).await;
	let entry = put(
		&app,
		&token,
		workspace,
		"restart",
		"A car carries passengers.",
		0,
	)
	.await;
	semantic::worker::sweep(&f.store).await.unwrap();
	assert_eq!(search(&app, &token, workspace).await.0, 200);
	// Act: restart the exact PostgreSQL instance owned by the application fixture.
	environment.await.restart_database().await;
	assert_eq!(
		search(&app, &token, workspace).await.1["matches"][0]["entry_id"],
		entry["id"],
		"source and acknowledged vector survive the same PostgreSQL restart"
	);
	let transport = aidash_server::bootstrap::semantic_transport(&f.store);
	transport
		.delete_point(
			&config.vector,
			index["collection"].as_str().unwrap(),
			Uuid::parse_str(entry["point_id"].as_str().unwrap()).unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(
		search(&app, &token, workspace).await.0,
		503,
		"missing vectors cannot masquerade as empty retrieval"
	);
	sqlx::query(
		&reinhardt::query::Query::update()
			.table(reinhardt::query::Alias::new("semantic_entries"))
			.value_expr(
				reinhardt::query::Alias::new("next_attempt"),
				reinhardt::query::Expr::cust("CLOCK_TIMESTAMP()"),
			)
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.execute(f.store.pool.driver())
	.await
	.unwrap();
	semantic::worker::sweep(&f.store).await.unwrap();
	assert_eq!(search(&app, &token, workspace).await.0, 200);
	drop(server);
	dispose(f, &url, &schema).await;
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::SimpleExpr;

use reinhardt::query::Expr;

#[rstest::fixture]
fn embeddings_router() -> std::sync::Arc<Router> {
	std::sync::Arc::new(reinhardt::test::stub::StubRouter::new()
.route("/v1/embeddings", http::Method::POST, reply(|request: reinhardt::Request| {let input = request.json::<Value>().unwrap();async move {
            let text=input["input"].as_str().unwrap_or_default().to_lowercase();
            // Explicit fixture semantics: related concepts share a vector even
            // when the query and stored text have no common keyword.
            let vector=if text.contains("car") || text.contains("vehicle") {vec![1.0,0.0,0.0]} else if text.contains("bread") || text.contains("baking") {vec![0.0,1.0,0.0]} else {vec![0.0,0.0,1.0]};
            reinhardt::Response::ok().with_json(&json!({"model":input["model"],"data":[{"index":0,"embedding":vector}],"usage":{"prompt_tokens":1,"total_tokens":1}})).unwrap()
        }})).into_server_router())
}
#[rstest::fixture]
async fn embeddings(
	#[from(embeddings_router)] _embeddings_router: std::sync::Arc<Router>,
	#[future(awt)]
	#[from(upstream::upstream)]
	#[with(_embeddings_router.clone())]
	server: reinhardt::test::fixtures::server::TestServerGuard,
) -> std::sync::Arc<reinhardt::test::fixtures::server::TestServerGuard> {
	std::sync::Arc::new(server)
}

#[rstest::fixture]
fn isolated_application(
	#[from(common::isolated_test_environment)] environment: futures_util::future::Shared<
		futures_util::future::BoxFuture<'static, std::sync::Arc<common::TestEnvironment>>,
	>,
	#[from(common::execution_database)]
	#[with(environment.clone())]
	database: common::DatabaseFuture,
	#[from(common::runtime)]
	#[with(database.clone())]
	runtime: common::RuntimeFuture,
	#[from(common::native_application)]
	#[with(Default::default(),aidash_server::sse::Service::new(Default::default()),Arc::new(|r|r),runtime.clone())]
	application: common::ApplicationFuture,
) -> common::ApplicationFuture {
	let _ = (environment, database, runtime);
	application
}

#[derive(Clone)]
struct ControlledContextState {
	captured: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
}
#[fixture]
fn controlled_context_state() -> ControlledContextState {
	ControlledContextState {
		captured: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
	}
}
#[fixture]
fn controlled_context_router(
	#[from(controlled_context_state)] state: ControlledContextState,
) -> std::sync::Arc<Router> {
	let captured = state.captured.clone();
	let received = captured.clone();
	std::sync::Arc::new(reinhardt::test::stub::StubRouter::new()
.route("/v1/chat/completions", http::Method::POST, reply(move |request: reinhardt::Request| {let body = request.json::<Value>().unwrap();
			let received = received.clone();
			async move {
				let context: Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
				received.lock().unwrap().push(context);
				reinhardt::Response::ok().with_json(&json!({"choices":[{"finish_reason":"stop","message":{"content":"Complete"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
			}
		})).into_server_router())
}
struct ControlledContextProvider {
	state: ControlledContextState,
	server: reinhardt::test::fixtures::server::TestServerGuard,
}
#[fixture]
async fn controlled_context_provider(
	#[from(controlled_context_state)] state: ControlledContextState,
	#[from(controlled_context_router)]
	#[with(state.clone())]
	_router: std::sync::Arc<Router>,
	#[future(awt)]
	#[from(upstream::upstream)]
	#[with(_router.clone())]
	server: reinhardt::test::fixtures::server::TestServerGuard,
) -> ControlledContextProvider {
	ControlledContextProvider { state, server }
}

#[derive(Clone)]
struct SemanticContextIsProvenancedAndRevocationHidesRunJournalsState {
	captured: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
}
#[fixture]
fn semantic_context_is_provenanced_and_revocation_hides_run_journals_state()
-> SemanticContextIsProvenancedAndRevocationHidesRunJournalsState {
	SemanticContextIsProvenancedAndRevocationHidesRunJournalsState {
		captured: Arc::new(Mutex::new(Vec::<Value>::new())),
	}
}
#[fixture]
fn semantic_context_is_provenanced_and_revocation_hides_run_journals_router(
	#[from(semantic_context_is_provenanced_and_revocation_hides_run_journals_state)]
	state: SemanticContextIsProvenancedAndRevocationHidesRunJournalsState,
) -> std::sync::Arc<Router> {
	let captured = state.captured.clone();
	let requests = captured.clone();
	std::sync::Arc::new(reinhardt::test::stub::StubRouter::new()
.route("/v1/chat/completions", http::Method::POST, reply(move |request: reinhardt::Request| {let body = request.json::<Value>().unwrap();
            let requests=requests.clone();
            async move {
                let context:Value=serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
                requests.lock().unwrap().push(context);
                reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"remember-car","type":"function","function":{"name":"workspace_observe","arguments":"{}"}}]}}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
            }
        })).into_server_router())
}
struct SemanticContextIsProvenancedAndRevocationHidesRunJournalsProvider {
	state: SemanticContextIsProvenancedAndRevocationHidesRunJournalsState,
	server: reinhardt::test::fixtures::server::TestServerGuard,
}
#[fixture]
async fn semantic_context_is_provenanced_and_revocation_hides_run_journals_provider(
	#[from(semantic_context_is_provenanced_and_revocation_hides_run_journals_state)]
	state: SemanticContextIsProvenancedAndRevocationHidesRunJournalsState,
	#[from(semantic_context_is_provenanced_and_revocation_hides_run_journals_router)]
	#[with(state.clone())]
	_router: std::sync::Arc<Router>,
	#[future(awt)]
	#[from(upstream::upstream)]
	#[with(_router.clone())]
	server: reinhardt::test::fixtures::server::TestServerGuard,
) -> SemanticContextIsProvenancedAndRevocationHidesRunJournalsProvider {
	SemanticContextIsProvenancedAndRevocationHidesRunJournalsProvider { state, server }
}
