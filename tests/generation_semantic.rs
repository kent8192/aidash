mod common;
use common::{TestEnvironment, test_environment};

use aidash::{api, federation::Federation, harness::Harness, semantic};
use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
use common::{bootstrap, cleanup, request, setup};
use serde_json::{Value, json};
use std::sync::{
	Arc,
	atomic::{AtomicUsize, Ordering},
};
use uuid::Uuid;

struct Fixture {
	f: Federation,
	url: String,
	schema: String,
	app: Router,
	token: String,
	workspace: Uuid,
	index: Value,
	job: Value,
	embeddings: Arc<AtomicUsize>,
	inference: Arc<AtomicUsize>,
	response_mode: Arc<AtomicUsize>,
	remember: Arc<AtomicUsize>,
	embedding_started: Arc<tokio::sync::Notify>,
	server: tokio::task::JoinHandle<()>,
}
impl Fixture {
	async fn new(allowance: Option<i64>, usage: Option<u64>) -> Self {
		let (f, url, schema) = setup().await;
		let embeddings = Arc::new(AtomicUsize::new(0));
		let inference = Arc::new(AtomicUsize::new(0));
		let embedding_calls = embeddings.clone();
		let model_calls = inference.clone();
		let response_mode = Arc::new(AtomicUsize::new(0));
		let embedding_mode = response_mode.clone();
		let remember = Arc::new(AtomicUsize::new(0));
		let model_mode = remember.clone();
		let embedding_started = Arc::new(tokio::sync::Notify::new());
		let started = embedding_started.clone();
		let pool = f.store.pool.clone();
		let provider = Router::new()
        .route(
            "/v1/embeddings",
            post(move |Json(input): Json<Value>| {
                let calls = embedding_calls.clone();
                let pool = pool.clone();
                let mode = embedding_mode.clone();
                let started = started.clone();
                async move {
                    let before = calls.fetch_add(1, Ordering::SeqCst);
                    if before > 0 && allowance.is_some() && input["input"] != "A car carries passengers." {
                        let attempts: i64 = sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::Expr::cust("COUNT(*)")).from(sea_orm::sea_query::Alias::new("generation_embedding_usage")).to_string(sea_orm::sea_query::PostgresQueryBuilder)).fetch_one(&pool).await.unwrap();
                        assert!(attempts > 0, "the attempt must be durable before HTTP");
                        let charged: i64 = sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("used_tokens")))).from(sea_orm::sea_query::Alias::new("generation_budgets")).limit(1).to_string(sea_orm::sea_query::PostgresQueryBuilder)).fetch_one(&pool).await.unwrap();
                        assert!(charged >= 1024, "input tokens must be reserved before HTTP");
                    }
                    if mode.load(Ordering::SeqCst) == 1 {
                        return StatusCode::SERVICE_UNAVAILABLE.into_response();
                    }
                    if mode.load(Ordering::SeqCst) == 2 {
                        started.notify_one();
                        std::future::pending::<()>().await;
                    }
                    Json(json!({"model":"fixture-embedding","data":[{"index":0,"embedding":[1.0,0.0,0.0]}],"usage":usage.map(|tokens| json!({"prompt_tokens":tokens,"total_tokens":tokens}))})).into_response()
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move || {
                let calls = model_calls.clone();
                let remember = model_mode.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    let (name, arguments) = if remember.load(Ordering::SeqCst) == 1 {
                        ("memory_write", "{\"note\":\"Generated vehicle findings\"}")
                    } else { ("workspace_observe", "{}") };
                    Json(json!({"choices":[{"index":0,"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"observe","type":"function","function":{"name":name,"arguments":arguments}}]}}],"usage":{"prompt_tokens":10,"completion_tokens":2}}))
                }
            }),
        );
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let endpoint = format!("http://{}", listener.local_addr().unwrap());
		let server = tokio::spawn(async move { axum::serve(listener, provider).await.unwrap() });
		let app = api::router(f.clone());
		let (_, token, original_task) = bootstrap(&f, &app, &endpoint).await;
		let workspace = f.store.task(original_task).await.unwrap().workspace_id;
		let (_, mut template) = request(
			&app,
			&f.config.api_token,
			"GET",
			"/api/registry/research/1.0.0",
			Value::Null,
		)
		.await;
		template["id"] = json!("semantic-template");
		template["capabilities"] = json!(["semantic.research"]);
		if allowance.is_some() {
			let (status, response) = request(&app, &f.config.api_token, "POST", "/api/registry", json!({"id":"embedding","version":"1.0.0","kind":"embedding","name":{"en":"Approved embedding"},"description":{"en":"Local embedding fixture"},"config":{"provider":"openai","endpoint":format!("{endpoint}/v1"),"credential_env":null,"model":"fixture-embedding","model_version":"1","dimensions":3}})).await;
			assert_eq!(status, 200, "{response}");
			assert_eq!(
				request(
					&app,
					&f.config.api_token,
					"POST",
					"/api/authorization/acme/catalog",
					json!({"entry":{"id":"embedding","version":"1.0.0"},"expected_revision":0,"enabled":true})
				)
				.await
				.0,
				200
			);
		}
		let spec = json!({"enabled":true,"template":template,"permissions":{"roles":[],"groups":[],"attributes":{}},"approval_required":false,"limits":{"max_agents":2,"max_concurrent":2,"max_depth":2,"token_budget":1000000,"tokens_per_agent":400000,"lifetime_seconds":3600},"embedding":allowance.map(|calls| json!({"provider":{"id":"embedding","version":"1.0.0"},"calls_per_agent":calls,"call_budget":calls*2}))});
		let (status, response) = request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/generation/acme/policies/semantic",
			json!({"expected_revision":0,"spec":spec}),
		)
		.await;
		assert_eq!(status, 200, "{response}");
		let (status, index) = request(&app,&f.config.api_token,"POST",&format!("/api/workspaces/{workspace}/semantic/index"),json!({"expected_revision":0,"spec":{"embedding":{"provider":"openai","endpoint":format!("{endpoint}/v1"),"credential_env":null,"model":"fixture-embedding","model_version":"1","dimensions":3},"vector":{"provider":"qdrant","endpoint":std::env::var("AIDASH_TEST_QDRANT_URL").unwrap(),"credential_env":"AIDASH_SECRET_TEST_QDRANT"},"enabled":true,"auto_context":true,"max_sources":64,"max_results":10,"max_result_tokens":4096,"max_input_bytes":8192}})).await;
		assert_eq!(status, 200, "{index}");
		assert_eq!(request(&app,&token,"POST",&format!("/api/workspaces/{workspace}/semantic/entries"),json!({"key":"reference","expected_revision":0,"source":{"kind":"memory","text":"A car carries passengers."},"metadata":{}})).await.0,200);
		semantic::worker::sweep(&f.store).await.unwrap();
		assert_eq!(embeddings.load(Ordering::SeqCst), 1);
		let (status, task) = request(&app,&token,"POST",&format!("/api/workspaces/{workspace}/tasks"),json!({"title":"Vehicle research","description":"Use related memory","requirements":{"capability":"semantic.research"}})).await;
		assert_eq!(status, 200, "{task}");
		let task = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
		let (status, assignment) = request(
			&app,
			&token,
			"POST",
			&format!("/api/generation/acme/tasks/{task}/assign"),
			json!({"policy_id":"semantic","reason":"missing specialist"}),
		)
		.await;
		assert_eq!(status, 200, "{assignment}");
		assert_eq!(assignment["kind"], "generated");
		aidash::generation::provision::reconcile(&f).await.unwrap();
		Harness {
			federation: f.clone(),
		}
		.worker_once()
		.await
		.unwrap();
		Self {
			f,
			url,
			schema,
			app,
			token,
			workspace,
			index,
			job: assignment["generation"].clone(),
			embeddings,
			inference,
			response_mode,
			remember,
			embedding_started,
			server,
		}
	}
	async fn drive(&self) {
		// Several real database/provider boundaries run concurrently under
		// coverage in CI. This fixture deadline is not a runtime latency SLO.
		let settled = tokio::time::timeout(std::time::Duration::from_secs(20), async {
			let worker = Harness {
				federation: self.f.clone(),
			};
			loop {
				worker.worker_once().await.unwrap();
				let run = self
					.f
					.store
					.runs()
					.await
					.unwrap()
					.into_iter()
					.find(|run| run.agent_id == self.job["agent_id"].as_str().unwrap())
					.unwrap();
				if matches!(run.phase.as_str(), "COMPLETED" | "FAILED" | "CANCELLED")
					|| run.control == "PAUSED"
				{
					break;
				}
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		})
		.await;
		if settled.is_err() {
			let states: Vec<_> = self
				.f
				.store
				.runs()
				.await
				.unwrap()
				.into_iter()
				.map(|run| (run.agent_id, run.phase, run.control))
				.collect();
			panic!(
				"generated execution did not settle in 20s: {states:?}; embedding calls={}, inference calls={}",
				self.embeddings.load(Ordering::SeqCst),
				self.inference.load(Ordering::SeqCst)
			);
		}
	}
	async fn remember(&self) -> Uuid {
		self.remember.store(1, Ordering::SeqCst);
		tokio::time::timeout(std::time::Duration::from_secs(5), async {
			let worker = Harness {
				federation: self.f.clone(),
			};
			loop {
				worker.worker_once().await.unwrap();
				let entry: Option<Uuid> = sqlx::query_scalar(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::SimpleExpr::from(
							sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new(
								"entry_id",
							)),
						))
						.from(sea_orm::sea_query::Alias::new("semantic_agent_memory"))
						.limit(1)
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.fetch_optional(&self.f.store.pool)
				.await
				.unwrap();
				if let Some(entry) = entry {
					break entry;
				}
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("memory_write must persist the generated authority without deadlock")
	}
	async fn usage(&self) -> Value {
		let (status, value) = request(
			&self.app,
			&self.token,
			"GET",
			&format!(
				"/api/generation/acme/requests/{}/usage",
				self.job["id"].as_str().unwrap()
			),
			Value::Null,
		)
		.await;
		assert_eq!(status, 200, "{value}");
		value
	}
	async fn dispose(self) {
		let collections: Vec<(String, Value)> = sqlx::query_as(
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
		.fetch_all(&self.f.store.pool)
		.await
		.unwrap();
		for (collection, config) in collections {
			semantic::backend::delete_collection(
				&self.f.store.semantic_client,
				&serde_json::from_value(config).unwrap(),
				&collection,
			)
			.await
			.unwrap();
		}
		self.server.abort();
		cleanup(self.f, &self.url, &self.schema).await;
	}
}

#[rstest::rstest]
#[tokio::test]
async fn generated_semantic_context_without_embedding_approval_never_calls_provider(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let fixture = Fixture::new(None, Some(2)).await;
	fixture.drive().await;
	let observed = (
		fixture.embeddings.load(Ordering::SeqCst),
		fixture.inference.load(Ordering::SeqCst),
	);
	fixture.dispose().await;
	assert_eq!(
		observed,
		(1, 0),
		"a workspace index cannot authorize or fund a generated Agent's embedding request"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn generated_embeddings_are_pinned_reserved_and_limited_with_reported_usage(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let fixture = Fixture::new(Some(2), Some(2)).await;
	let (_, policies) = request(
		&fixture.app,
		&fixture.f.config.api_token,
		"GET",
		"/api/generation/acme/policies",
		Value::Null,
	)
	.await;
	let mut spec = policies[0]["spec"].clone();
	spec["embedding"] = Value::Null;
	let (status, value) = request(
		&fixture.app,
		&fixture.f.config.api_token,
		"POST",
		"/api/generation/acme/policies/semantic",
		json!({"expected_revision":1,"spec":spec}),
	)
	.await;
	assert_eq!(status, 200, "{value}");
	fixture.drive().await;
	assert_eq!(fixture.embeddings.load(Ordering::SeqCst), 3);
	assert_eq!(fixture.inference.load(Ordering::SeqCst), 2);
	let usage = fixture.usage().await;
	assert_eq!(usage["embedding_calls"], 2);
	assert_eq!(usage["embedding_call_limit"], 2);
	assert_eq!(
		usage["used_tokens"], 28,
		"two embedding and inference results refund only verified unused tokens"
	);
	aidash::generation::provision::reconcile(&fixture.f)
		.await
		.unwrap();
	let (_, policies) = request(
		&fixture.app,
		&fixture.f.config.api_token,
		"GET",
		"/api/generation/acme/policies",
		Value::Null,
	)
	.await;
	assert_eq!(policies[0]["allocated_embedding_calls"], 2);
	assert_eq!(policies[0]["allocated_tokens"], 28);
	fixture.dispose().await;
}

#[rstest::rstest]
#[tokio::test]
async fn missing_embedding_usage_retains_input_reservation_and_expired_agents_make_no_call(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let fixture = Fixture::new(Some(1), None).await;
	fixture.drive().await;
	let reserved: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("reserved_tokens")),
			))
			.from(sea_orm::sea_query::Alias::new("generation_embedding_usage"))
			.limit(1)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&fixture.f.store.pool)
	.await
	.unwrap();
	let usage = fixture.usage().await;
	assert_eq!(usage["embedding_calls"], 1);
	assert_eq!(usage["used_tokens"], reserved + 12);
	assert_eq!(fixture.embeddings.load(Ordering::SeqCst), 2);
	fixture.dispose().await;
	let fixture = Fixture::new(Some(2), Some(2)).await;
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("generation_requests"))
			.value(
				sea_orm::sea_query::Alias::new("expires_at"),
				sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP() - INTERVAL '1 SECOND'"),
			)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.execute(&fixture.f.store.pool)
	.await
	.unwrap();
	fixture.drive().await;
	assert_eq!(fixture.embeddings.load(Ordering::SeqCst), 1);
	assert_eq!(fixture.inference.load(Ordering::SeqCst), 0);
	assert_eq!(fixture.usage().await["embedding_calls"], 0);
	fixture.dispose().await;
}

#[rstest::rstest]
#[tokio::test]
async fn generated_embedding_catalog_policy_and_provider_changes_cannot_increase_authority(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	for reason in ["catalog", "policy", "index"] {
		let mut fixture = Fixture::new(Some(2), Some(2)).await;
		if reason == "catalog" {
			assert_eq!(
				request(
					&fixture.app,
					&fixture.f.config.api_token,
					"POST",
					"/api/authorization/acme/catalog",
					json!({"entry":{"id":"embedding","version":"1.0.0"},"expected_revision":1,"enabled":false})
				)
				.await
				.0,
				200
			);
		} else if reason == "policy" {
			let authorization = aidash::authorization::Authorization {
				pool: fixture.f.store.pool.clone(),
			};
			let snapshot = authorization.snapshot("acme").await.unwrap();
			let mut bundle = json!(snapshot.bundle);
			bundle["policies"].as_array_mut().unwrap().push(json!({"id":"deny-embedding","effect":"deny","subjects":{"ids":["alice"]},"actions":["embedding.invoke"],"resources":{"kinds":["embedding"]}}));
			authorization
				.replace(
					"acme",
					snapshot.revision,
					serde_json::from_value(bundle).unwrap(),
					"operator",
				)
				.await
				.unwrap();
		} else {
			let mut spec = fixture.index["spec"].clone();
			spec["embedding"]["model_version"] = json!("2");
			let (status, index) = request(
				&fixture.app,
				&fixture.f.config.api_token,
				"POST",
				&format!("/api/workspaces/{}/semantic/index", fixture.workspace),
				json!({"expected_revision":1,"spec":spec}),
			)
			.await;
			assert_eq!(status, 200, "{index}");
			fixture.index = index;
			semantic::worker::sweep(&fixture.f.store).await.unwrap();
		}
		let before = fixture.embeddings.load(Ordering::SeqCst);
		fixture.drive().await;
		assert_eq!(
			fixture.embeddings.load(Ordering::SeqCst),
			before,
			"{reason}"
		);
		assert_eq!(fixture.inference.load(Ordering::SeqCst), 0, "{reason}");
		assert_eq!(fixture.usage().await["embedding_calls"], 0, "{reason}");
		fixture.dispose().await;
	}
}

#[rstest::rstest]
#[tokio::test]
async fn background_indexing_retains_failed_charges_across_recovery_and_cannot_overspend(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let fixture = Fixture::new(Some(2), Some(2)).await;
	let entry = fixture.remember().await;
	assert_eq!(fixture.usage().await["embedding_calls"], 1);
	fixture.response_mode.store(1, Ordering::SeqCst);
	tokio::time::timeout(std::time::Duration::from_secs(5), async {
		let (first, second) = tokio::join!(
			semantic::worker::sweep(&fixture.f.store),
			semantic::worker::sweep(&fixture.f.store)
		);
		first.unwrap();
		second.unwrap();
	})
	.await
	.expect("concurrent indexers must not deadlock the durable reservation");
	assert_eq!(fixture.embeddings.load(Ordering::SeqCst), 3);
	let (reserved, reported): (i64, Option<i64>) = sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("reserved_tokens")),
			))
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("reported_tokens")),
			))
			.from(sea_orm::sea_query::Alias::new("generation_embedding_usage"))
			.and_where(sea_orm::sea_query::Expr::cust(
				"purpose = 'index' AND entry_id = $1",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(entry)
	.fetch_one(&fixture.f.store.pool)
	.await
	.unwrap();
	assert_eq!(reported, None);
	assert_eq!(fixture.usage().await["used_tokens"], reserved + 14);
	fixture.response_mode.store(0, Ordering::SeqCst);
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
	.bind(entry)
	.execute(&fixture.f.store.pool)
	.await
	.unwrap();
	let recovered = fixture.f.store.isolated_pool().await.unwrap();
	semantic::worker::sweep(&recovered).await.unwrap();
	recovered.pool.close().await;
	recovered.control_pool.close().await;
	let (state, attempts): (String, i32) = sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("state")),
			))
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("attempts")),
			))
			.from(sea_orm::sea_query::Alias::new("semantic_entries"))
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(entry)
	.fetch_one(&fixture.f.store.pool)
	.await
	.unwrap();
	assert_eq!((state.as_str(), attempts), ("ERROR", 2));
	assert_eq!(
		fixture.embeddings.load(Ordering::SeqCst),
		3,
		"an uncertain failed attempt consumes the remaining call allowance"
	);
	assert_eq!(fixture.usage().await["embedding_calls"], 2);
	fixture.dispose().await;
}

#[rstest::rstest]
#[tokio::test]
async fn background_indexing_uses_generated_authority_and_checks_expiry_before_http(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	for expired in [false, true] {
		let fixture = Fixture::new(Some(2), Some(2)).await;
		let entry = fixture.remember().await;
		if expired {
			sqlx::query(
				&sea_orm::sea_query::Query::update()
					.table(sea_orm::sea_query::Alias::new("generation_requests"))
					.value(
						sea_orm::sea_query::Alias::new("expires_at"),
						sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP() - INTERVAL '1 SECOND'"),
					)
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.execute(&fixture.f.store.pool)
			.await
			.unwrap();
		}
		tokio::time::timeout(
			std::time::Duration::from_secs(5),
			semantic::worker::sweep(&fixture.f.store),
		)
		.await
		.expect("indexing must not block on a source held by its own authority lease")
		.unwrap();
		let state: String = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("state")),
				))
				.from(sea_orm::sea_query::Alias::new("semantic_entries"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(entry)
		.fetch_one(&fixture.f.store.pool)
		.await
		.unwrap();
		assert_eq!(state, if expired { "ERROR" } else { "READY" });
		assert_eq!(
			fixture.embeddings.load(Ordering::SeqCst),
			if expired { 2 } else { 3 }
		);
		assert_eq!(
			fixture.usage().await["embedding_calls"],
			if expired { 1 } else { 2 }
		);
		assert_eq!(
			fixture.usage().await["used_tokens"],
			if expired { 14 } else { 16 }
		);
		fixture.dispose().await;
	}
}

#[rstest::rstest]
#[tokio::test]
async fn excessive_embedding_usage_retains_reservation_and_prevents_inference(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let fixture = Fixture::new(Some(2), Some(1_000_000)).await;
	fixture.drive().await;
	let reserved: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("reserved_tokens")),
			))
			.from(sea_orm::sea_query::Alias::new("generation_embedding_usage"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&fixture.f.store.pool)
	.await
	.unwrap();
	assert_eq!(fixture.embeddings.load(Ordering::SeqCst), 2);
	assert_eq!(fixture.inference.load(Ordering::SeqCst), 0);
	assert_eq!(fixture.usage().await["used_tokens"], reserved);
	fixture.dispose().await;
}

#[rstest::rstest]
#[tokio::test]
async fn nested_embeddings_intersect_pinned_providers_and_charge_each_ancestor(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	for case in ["unapproved", "approved", "token_exhausted"] {
		let approved = case == "approved";
		let mut fixture = Fixture::new(Some(1), Some(2)).await;
		let parent = fixture.job.clone();
		let parent_run = fixture.f.store.runs().await.unwrap().remove(0);
		fixture
			.f
			.store
			.control(parent_run.id, "pause")
			.await
			.unwrap();
		if case == "unapproved" {
			let (_, policies) = request(
				&fixture.app,
				&fixture.token,
				"GET",
				"/api/generation/acme/policies",
				Value::Null,
			)
			.await;
			let mut spec = policies[0]["spec"].clone();
			spec["embedding"] = Value::Null;
			assert_eq!(
				request(
					&fixture.app,
					&fixture.token,
					"POST",
					"/api/generation/acme/policies/semantic",
					json!({"expected_revision":1,"spec":spec})
				)
				.await
				.0,
				200
			);
		}
		let (status, task) = request(&fixture.app, &fixture.token, "POST", &format!("/api/workspaces/{}/tasks", fixture.workspace), json!({"title":"Child specialist","description":"Inherited embedding budget","requirements":{"capability":"semantic.research"}})).await;
		assert_eq!(status, 200);
		let task: Uuid = task["id"].as_str().unwrap().parse().unwrap();
		// This is the trusted origin produced by task_create, whose worker
		// transport is tested in generation.rs. Exercise the budget intersection.
		let chain = vec![
			"alice".to_owned(),
			aidash::domain::qualified_agent(
				&fixture.f.config.node_id,
				&parent_run.agent_id,
				&parent_run.agent_version,
			),
		];
		sqlx::query(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("authorization_task_origins"))
				.columns([
					sea_orm::sea_query::Alias::new("task_id"),
					sea_orm::sea_query::Alias::new("source_run_id"),
					sea_orm::sea_query::Alias::new("tenant"),
					sea_orm::sea_query::Alias::new("root_subject"),
					sea_orm::sea_query::Alias::new("subject_chain"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("'acme'"),
					sea_orm::sea_query::Expr::cust("'alice'"),
					sea_orm::sea_query::Expr::cust("$3"),
				])
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task)
		.bind(parent_run.id)
		.bind(chain)
		.execute(&fixture.f.store.pool)
		.await
		.unwrap();
		let (status, child) = request(
			&fixture.app,
			&fixture.token,
			"POST",
			&format!("/api/generation/acme/tasks/{task}/assign"),
			json!({"policy_id":"semantic","reason":"nested work"}),
		)
		.await;
		assert_eq!(status, 200, "{child}");
		fixture.job = child["generation"].clone();
		aidash::generation::provision::reconcile(&fixture.f)
			.await
			.unwrap();
		if case == "token_exhausted" {
			sqlx::query(
				&sea_orm::sea_query::Query::update()
					.table(sea_orm::sea_query::Alias::new("generation_budgets"))
					.value(
						sea_orm::sea_query::Alias::new("used_tokens"),
						sea_orm::sea_query::Expr::cust("token_limit - 1"),
					)
					.and_where(sea_orm::sea_query::Expr::cust("request_id = $1"))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(parent["id"].as_str().unwrap().parse::<Uuid>().unwrap())
			.execute(&fixture.f.store.pool)
			.await
			.unwrap();
		}
		fixture.drive().await;
		assert_eq!(
			fixture.embeddings.load(Ordering::SeqCst),
			1 + usize::from(approved)
		);
		assert_eq!(
			fixture.inference.load(Ordering::SeqCst),
			usize::from(approved)
		);
		assert_eq!(
			fixture.usage().await["embedding_calls"],
			i64::from(approved)
		);
		assert_eq!(
			fixture.usage().await["used_tokens"],
			if approved { 14 } else { 0 }
		);
		fixture.job = parent;
		assert_eq!(
			fixture.usage().await["embedding_calls"],
			i64::from(approved)
		);
		assert_eq!(
			fixture.usage().await["used_tokens"],
			if approved {
				14
			} else if case == "token_exhausted" {
				399999
			} else {
				0
			}
		);
		fixture.dispose().await;
	}
}

struct WorkerProcess(std::process::Child);
impl WorkerProcess {
	fn start(fixture: &Fixture) -> Self {
		let mut database = reqwest::Url::parse(&fixture.url).unwrap();
		database
			.query_pairs_mut()
			.append_pair("options", &format!("-c search_path={}", fixture.schema));
		Self(
			std::process::Command::new(env!("CARGO_BIN_EXE_aidash"))
				.arg("worker")
				.env("DATABASE_URL", database.as_str())
				.env("AIDASH_NODE_ID", &fixture.f.config.node_id)
				.env("AIDASH_ENDPOINT", &fixture.f.config.endpoint)
				.env("AIDASH_API_TOKEN", &fixture.f.config.api_token)
				.env(
					"NATS_URL",
					std::env::var("AIDASH_TEST_NATS_URL")
						.unwrap_or_else(|_| "nats://127.0.0.1:42270".into()),
				)
				.env("RUST_LOG", "aidash=warn")
				.spawn()
				.unwrap(),
		)
	}
}
impl Drop for WorkerProcess {
	fn drop(&mut self) {
		let _ = self.0.kill();
		let _ = self.0.wait();
	}
}

#[rstest::rstest]
#[tokio::test]
async fn killed_embedding_worker_retains_uncertain_usage_and_restart_reserves_a_new_attempt(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let fixture = Fixture::new(Some(2), Some(2)).await;
	fixture.response_mode.store(2, Ordering::SeqCst);
	let mut worker = WorkerProcess::start(&fixture);
	let reached = tokio::time::timeout(
		std::time::Duration::from_secs(20),
		fixture.embedding_started.notified(),
	)
	.await;
	assert!(
		reached.is_ok(),
		"embedding provider not reached; worker status {:?}",
		worker.0.try_wait()
	);
	drop(worker); // Real SIGKILL, before the provider returns usage or a vector.
	let reserved: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("reserved_tokens")),
			))
			.from(sea_orm::sea_query::Alias::new("generation_embedding_usage"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&fixture.f.store.pool)
	.await
	.unwrap();
	assert_eq!(fixture.usage().await["used_tokens"], reserved);
	assert_eq!(fixture.usage().await["embedding_calls"], 1);
	fixture.response_mode.store(0, Ordering::SeqCst);
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("runs"))
			.value(
				sea_orm::sea_query::Alias::new("lease_until"),
				sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP() - INTERVAL '1 SECOND'"),
			)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.execute(&fixture.f.store.pool)
	.await
	.unwrap();
	let worker = WorkerProcess::start(&fixture);
	tokio::time::timeout(std::time::Duration::from_secs(20), async {
		loop {
			let run = fixture.f.store.runs().await.unwrap().remove(0);
			if run.phase == "FAILED" {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(50)).await;
		}
	})
	.await
	.expect("new worker must recover the original run and enforce the retained call limit");
	drop(worker);
	assert_eq!(fixture.f.store.runs().await.unwrap().len(), 1);
	assert_eq!(fixture.embeddings.load(Ordering::SeqCst), 3);
	assert_eq!(fixture.inference.load(Ordering::SeqCst), 1);
	assert_eq!(fixture.usage().await["embedding_calls"], 2);
	assert_eq!(fixture.usage().await["used_tokens"], reserved + 14);
	let attempts: (i64, i64) = sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
			.expr(sea_orm::sea_query::Expr::cust("COUNT(reported_tokens)"))
			.from(sea_orm::sea_query::Alias::new("generation_embedding_usage"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&fixture.f.store.pool)
	.await
	.unwrap();
	assert_eq!(attempts, (2, 1));
	fixture.dispose().await;
}
