mod common;
use aidash::{
	api,
	semantic::{self, ConfigureIndex, EmbeddingConfig, IndexSpec, VectorConfig},
};
use axum::{Json, Router, routing::post};
use common::request;
use common::{TestEnvironment, test_environment};
use serde_json::{Value, json};
use testcontainers::{
	ContainerAsync, GenericImage, ImageExt, core::IntoContainerPort, runners::AsyncRunner,
};
use uuid::Uuid;

struct RestartableQdrant {
	container: ContainerAsync<GenericImage>,
	endpoint: String,
}

#[rstest::fixture]
async fn restartable_qdrant() -> RestartableQdrant {
	use std::time::{Duration, Instant};

	// A restart test needs the endpoint to survive stop/start. Reserve a random
	// host port, then ask Testcontainers to keep that mapping for this container.
	let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
	let host_port = reservation.local_addr().unwrap().port();
	drop(reservation);
	let container = GenericImage::new("qdrant/qdrant", "v1.19.1")
		.with_mapped_port(host_port, 6333.tcp())
		.start()
		.await
		.expect("start restartable Qdrant fixture");
	let port = container
		.get_host_port_ipv4(6333)
		.await
		.expect("mapped Qdrant fixture port");
	let endpoint = format!("http://127.0.0.1:{port}");
	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(1))
		.build()
		.unwrap();
	let until = Instant::now() + Duration::from_secs(30);
	loop {
		if client
			.get(format!("{endpoint}/readyz"))
			.send()
			.await
			.is_ok_and(|response| response.status().is_success())
		{
			break;
		}
		assert!(Instant::now() < until, "test Qdrant startup timed out");
		tokio::time::sleep(Duration::from_millis(100)).await;
	}

	RestartableQdrant {
		container,
		endpoint,
	}
}

async fn embeddings() -> (String, tokio::task::JoinHandle<()>) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let url = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener,Router::new().route("/v1/embeddings",post(|Json(input):Json<Value>| async move {
            let text=input["input"].as_str().unwrap_or_default().to_lowercase();
            // Explicit fixture semantics: related concepts share a vector even
            // when the query and stored text have no common keyword.
            let vector=if text.contains("car") || text.contains("vehicle") {vec![1.0,0.0,0.0]} else if text.contains("bread") || text.contains("baking") {vec![0.0,1.0,0.0]} else {vec![0.0,0.0,1.0]};
            Json(json!({"model":input["model"],"data":[{"index":0,"embedding":vector}],"usage":{"prompt_tokens":1,"total_tokens":1}}))
        }))).await.unwrap();
	});
	(url, server)
}
fn spec(endpoint: &str, qdrant_url: &str) -> IndexSpec {
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
			provider: "qdrant".into(),
			endpoint: qdrant_url.into(),
			credential_env: Some("AIDASH_SECRET_TEST_QDRANT".into()),
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
	app: &Router,
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
	app: &Router,
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
async fn search(app: &Router, token: &str, workspace: Uuid) -> (u16, Value) {
	request(
		app,
		token,
		"POST",
		&format!("/api/workspaces/{workspace}/semantic/search"),
		json!({"query":"vehicle","limit":1,"max_tokens":2048}),
	)
	.await
}
async fn dispose(f: aidash::federation::Federation, url: &str, schema: &str) {
	let rows: Vec<(String, Value)> = sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("collection")),
			))
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("vector")),
			))
			.from(sea_orm::sea_query::Alias::new("semantic_collections"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_all(&f.store.pool)
	.await
	.unwrap();
	for (collection, config) in rows {
		semantic::backend::delete_collection(
			&f.store.semantic_client,
			&serde_json::from_value(config).unwrap(),
			&collection,
		)
		.await
		.unwrap();
	}
	common::cleanup(f, url, schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_lifecycle_is_durable_revisioned_and_not_keyword_search(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = common::setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (endpoint, server) = embeddings().await;
	let (_, token, task) = common::bootstrap(&f, &app, &endpoint).await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	let mut config = spec(&endpoint, &_test_environment.qdrant_url);
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
	let body: Value = reqwest::Client::new()
		.post(format!(
			"{}/collections/{}/points",
			config.vector.endpoint,
			second["collection"].as_str().unwrap()
		))
		.header(
			"api-key",
			std::env::var("AIDASH_SECRET_TEST_QDRANT").unwrap(),
		)
		.json(&json!({"ids":[current_point],"with_payload":true}))
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(body["result"], json!([]));
	server.abort();
	dispose(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_access_is_checked_before_search_and_jobs_retain_revocation(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = common::setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (endpoint, server) = embeddings().await;
	let (mut policy, token, task) = common::bootstrap(&f, &app, &endpoint).await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	configure(
		&app,
		&f.config.api_token,
		workspace,
		&spec(&endpoint, &_test_environment.qdrant_url),
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
	server.abort();
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
	let auth = aidash::authorization::Authorization {
		pool: f.store.pool.clone(),
	};
	auth.revoke_credential(
		"acme",
		Uuid::parse_str(credential["id"].as_str().unwrap()).unwrap(),
	)
	.await
	.unwrap();
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("semantic_entries"))
			.value(
				sea_orm::sea_query::Alias::new("next_attempt"),
				sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(Uuid::parse_str(cars["id"].as_str().unwrap()).unwrap())
	.execute(&f.store.pool)
	.await
	.unwrap();
	semantic::worker::sweep(&f.store).await.unwrap();
	let state: String = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("state")),
			))
			.from(sea_orm::sea_query::Alias::new("semantic_entries"))
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(Uuid::parse_str(cars["id"].as_str().unwrap()).unwrap())
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(state, "REVOKED");
	assert_eq!(search(&app, &token, workspace).await.0, 401);
	dispose(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_outage_and_input_bounds_are_visible_and_retriable(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = common::setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (endpoint, server) = embeddings().await;
	let (_, token, task) = common::bootstrap(&f, &app, &endpoint).await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	let mut config = spec(&endpoint, &_test_environment.qdrant_url);
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
	server.abort();
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
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	use std::sync::{Arc, Mutex};
	let (f, url, schema) = common::setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (embedding_endpoint, embedding_server) = embeddings().await;
	let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
	let requests = captured.clone();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener,Router::new().route("/v1/chat/completions",post(move |Json(body):Json<Value>| {
            let requests=requests.clone();
            async move {
                let context:Value=serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
                requests.lock().unwrap().push(context);
                Json(json!({"choices":[{"index":0,"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"remember-car","type":"function","function":{"name":"memory_write","arguments":"{\"note\":\"A car carries passengers safely.\"}"}}]}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
            }
        }))).await.unwrap();
	});
	let (mut policy, token, task) = common::bootstrap(&f, &app, &endpoint).await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	configure(
		&app,
		&f.config.api_token,
		workspace,
		&spec(&embedding_endpoint, &_test_environment.qdrant_url),
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
	let worker = aidash::harness::Harness {
		federation: f.for_workers().await.unwrap(),
	};
	worker.worker_once().await.unwrap();
	worker.worker_once().await.unwrap();
	let run = f.store.runs().await.unwrap().remove(0);
	assert_eq!(run.phase, "TOOL_CALL", "{:?}", run.error);
	let context = captured.lock().unwrap()[0].clone();
	assert_eq!(
		context["current"]["semantic_memory"]["matches"][0]["entry_id"],
		cars["id"]
	);
	assert_eq!(context["current"]["semantic_memory"]["model_version"], "1");
	let count: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
			.from(sea_orm::sea_query::Alias::new("semantic_run_reads"))
			.and_where(sea_orm::sea_query::Expr::cust("run_id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(run.id)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(count, 1);
	worker.worker_once().await.unwrap();
	assert_eq!(
		f.store.memory(&run).await.unwrap(),
		json!({"note":"A car carries passengers safely."})
	);
	let (agent, authority): (Option<String>, Value) = sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("agent")),
			))
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("authority")),
			))
			.from(sea_orm::sea_query::Alias::new("semantic_entries"))
			.and_where(sea_orm::sea_query::Expr::cust(
				"metadata ->> 'origin' = 'agent_memory'",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(
		agent,
		Some(aidash::domain::qualified_agent(
			&f.config.node_id,
			"research",
			"1.0.0"
		))
	);
	assert!(authority["credential"].is_string());
	assert_eq!(authority["subjects"].as_array().unwrap().len(), 2);
	semantic::worker::sweep(&f.store).await.unwrap();
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-original-memory","effect":"deny","subjects":{"ids":["alice"]},"actions":["memory.read"],"resources":{"kinds":["memory"]}}));
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
		1,
		"semantic reads retain the original Agent memory permission"
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
	let managed: Uuid = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("entry_id")),
			))
			.from(sea_orm::sea_query::Alias::new("semantic_agent_memory"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(
		request(
			&app,
			&token,
			"DELETE",
			&format!("/api/workspaces/{workspace}/semantic/entries/{managed}"),
			json!({"expected_revision":1})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		f.store.memory(&run).await.unwrap(),
		json!({}),
		"deleted Agent memory must not survive through the legacy context slot"
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
	assert_eq!(f.store.run(run.id).await.unwrap().control, "PAUSED");
	assert_eq!(captured.lock().unwrap().len(), 1);
	worker.federation.store.pool.close().await;
	worker.federation.store.control_pool.close().await;
	server.abort();
	embedding_server.abort();
	dispose(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn linked_sources_and_agent_metadata_filters_respect_original_authority(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	use aidash::domain::{ArtifactInput, qualified_agent};
	let (f, url, schema) = common::setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (endpoint, server) = embeddings().await;
	let (mut policy, token, task) = common::bootstrap(&f, &app, &endpoint).await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	configure(
		&app,
		&f.config.api_token,
		workspace,
		&spec(&endpoint, &_test_environment.qdrant_url),
		0,
	)
	.await;
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
	aidash::harness::Harness {
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
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("artifacts"))
			.value(
				sea_orm::sea_query::Alias::new("content"),
				sea_orm::sea_query::Expr::cust("$2"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(artifact.id)
	.bind(json!("A car needs brakes."))
	.execute(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(search(&app, &token, workspace).await.0, 409);
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("semantic_entries"))
			.value(
				sea_orm::sea_query::Alias::new("next_attempt"),
				sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()"),
			)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.execute(&f.store.pool)
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
	configure(
		&app,
		&f.config.api_token,
		other.id,
		&spec(&endpoint, &_test_environment.qdrant_url),
		0,
	)
	.await;
	assert_eq!(request(&app,&f.config.api_token,"POST",&format!("/api/workspaces/{}/semantic/entries",other.id),json!({"key":"forged","expected_revision":0,"source":{"kind":"artifact","id":artifact.id},"metadata":{}})).await.0,403);
	assert_eq!(search(&app, &token, other.id).await.0, 403);
	server.abort();
	dispose(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_qdrant_restart_outage_and_lost_points_recover(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
	#[future(awt)] restartable_qdrant: RestartableQdrant,
) {
	use std::time::{Duration, Instant};
	let RestartableQdrant {
		container,
		endpoint,
	} = restartable_qdrant;
	let (f, url, schema) = common::setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (embedding, server) = embeddings().await;
	let (_, token, task) = common::bootstrap(&f, &app, &embedding).await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	let mut config = spec(&embedding, &_test_environment.qdrant_url);
	config.vector.endpoint = endpoint.clone();
	config.vector.credential_env = None;
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
	container.stop_with_timeout(Some(1)).await.unwrap();
	container.start().await.unwrap();
	// Do not retain an HTTP connection pool across a deliberate process restart.
	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(1))
		.build()
		.unwrap();
	let until = Instant::now() + Duration::from_secs(30);
	loop {
		if client
			.get(format!("{endpoint}/readyz"))
			.send()
			.await
			.is_ok_and(|r| r.status().is_success())
		{
			break;
		}
		assert!(Instant::now() < until);
		tokio::time::sleep(Duration::from_millis(100)).await;
	}
	assert_eq!(
		search(&app, &token, workspace).await.1["matches"][0]["entry_id"],
		entry["id"],
		"Qdrant must retain the acknowledged point across process restart"
	);
	let collection = index["collection"].as_str().unwrap();
	semantic::backend::delete_point(
		&f.store.semantic_client,
		&config.vector,
		collection,
		Uuid::parse_str(entry["point_id"].as_str().unwrap()).unwrap(),
	)
	.await
	.unwrap();
	assert_eq!(
		search(&app, &token, workspace).await.0,
		503,
		"missing points must not become successful empty retrieval"
	);
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("semantic_entries"))
			.value(
				sea_orm::sea_query::Alias::new("next_attempt"),
				sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()"),
			)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.execute(&f.store.pool)
	.await
	.unwrap();
	semantic::worker::sweep(&f.store).await.unwrap();
	assert_eq!(search(&app, &token, workspace).await.0, 200);
	container.stop_with_timeout(Some(1)).await.unwrap();
	assert_eq!(search(&app, &token, workspace).await.0, 503);
	put(&app, &token, workspace, "restart", "A car uses roads.", 1).await;
	semantic::worker::sweep(&f.store).await.unwrap();
	let state: String = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("state")),
			))
			.from(sea_orm::sea_query::Alias::new("semantic_entries"))
			.limit(1)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(state, "ERROR");
	container.start().await.unwrap();
	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(1))
		.build()
		.unwrap();
	let until = Instant::now() + Duration::from_secs(30);
	loop {
		if client
			.get(format!("{endpoint}/readyz"))
			.send()
			.await
			.is_ok_and(|r| r.status().is_success())
		{
			break;
		}
		assert!(Instant::now() < until);
		tokio::time::sleep(Duration::from_millis(100)).await;
	}
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("semantic_entries"))
			.value(
				sea_orm::sea_query::Alias::new("next_attempt"),
				sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()"),
			)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.execute(&f.store.pool)
	.await
	.unwrap();
	semantic::worker::sweep(&f.store).await.unwrap();
	assert_eq!(
		search(&app, &token, workspace).await.1["matches"][0]["text"],
		"A car uses roads."
	);
	server.abort();
	dispose(f, &url, &schema).await;
}
