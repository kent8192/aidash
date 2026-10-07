#[path = "../../tests/support/worker_process.rs"]
mod worker_process;
use worker_process::WorkerProcess;
#[path = "../../tests/support/legacy.rs"]
mod common;
use common::{TestEnvironment, test_environment};

use aidash_server::{federation::Federation, harness::Harness, semantic};
use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
use common::{bootstrap, cleanup, request, setup};
use reinhardt::query::ExprTrait as _;
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
	app: common::TestApplication,
	token: String,
	workspace: Uuid,
	index: Value,
	job: Value,
	embeddings: Arc<AtomicUsize>,
	inference: Arc<AtomicUsize>,
	response_mode: Arc<AtomicUsize>,
	remember: Arc<AtomicUsize>,
	derivations: Arc<AtomicUsize>,
	maintenance_origins: Arc<AtomicUsize>,
	embedding_started: Arc<tokio::sync::Notify>,
	server: tokio::task::JoinHandle<()>,
	memory_recovery_directory: tempfile::TempDir,
}
impl Fixture {
	async fn new(
		environment: &TestEnvironment,
		allowance: Option<i64>,
		usage: Option<u64>,
	) -> Self {
		Self::new_with_maintenance(environment, allowance, usage, false).await
	}
	async fn new_with_maintenance(
		environment: &TestEnvironment,
		allowance: Option<i64>,
		usage: Option<u64>,
		maintenance: bool,
	) -> Self {
		let (mut f, url, schema) = setup(environment).await;
		let memory_recovery_directory = tempfile::tempdir().unwrap();
		f.store = aidash_server::semantic::services::memory_recovery::initialize(
			&f.store,
			memory_recovery_directory.path().to_owned(),
		)
		.await
		.unwrap();
		let embeddings = Arc::new(AtomicUsize::new(0));
		let inference = Arc::new(AtomicUsize::new(0));
		let embedding_calls = embeddings.clone();
		let model_calls = inference.clone();
		let response_mode = Arc::new(AtomicUsize::new(0));
		let embedding_mode = response_mode.clone();
		let remember = Arc::new(AtomicUsize::new(0));
		let model_mode = remember.clone();
		let derivations = Arc::new(AtomicUsize::new(0));
		let derived_calls = derivations.clone();
		let maintenance_origins = Arc::new(AtomicUsize::new(1));
		let expected_origins = maintenance_origins.clone();
		let derivation_mode = response_mode.clone();
		let model_pool = f.store.pool.driver().clone();
		let embedding_started = Arc::new(tokio::sync::Notify::new());
		let started = embedding_started.clone();
		let pool = f.store.pool.driver().clone();
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
                        let attempts: i64 = sqlx::query_scalar(&reinhardt::query::Query::select().expr(reinhardt::query::Expr::cust("COUNT(*)")).from(reinhardt::query::Alias::new("generation_embedding_usage")).to_string(reinhardt::query::PostgresQueryBuilder)).fetch_one(&pool).await.unwrap();
                        assert!(attempts > 0, "the attempt must be durable before HTTP");
                        let charged: i64 = sqlx::query_scalar(&reinhardt::query::Query::select().expr(reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(reinhardt::query::Alias::new("used_tokens")))).from(reinhardt::query::Alias::new("generation_budgets")).limit(1).to_string(reinhardt::query::PostgresQueryBuilder)).fetch_one(&pool).await.unwrap();
                        assert!(charged >= 1024, "input tokens must be reserved before HTTP");
                    }
                    if mode.load(Ordering::SeqCst) == 1 {
                        return StatusCode::SERVICE_UNAVAILABLE.into_response();
                    }
                    if mode.load(Ordering::SeqCst) == 2 {
                        started.notify_one();
                        std::future::pending::<()>().await;
                    }
                    Json(json!({"model":"fixture-embedding","data":[{"index":0,"embedding":[1.0,0.0,0.0]}],"usage":(if before == 0 { Some(1) } else { usage }).map(|tokens| json!({"prompt_tokens":tokens,"total_tokens":tokens}))})).into_response()
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move |Json(input): Json<Value>| {
                let calls = model_calls.clone();
                let remember = model_mode.clone();
                let derived = derived_calls.clone();
                let mode = derivation_mode.clone();
                let pool = model_pool.clone();
                let expected_origins = expected_origins.clone();
                async move {
                    if let Some(context) = input["messages"][1]["content"].as_str().and_then(|value| serde_json::from_str::<Value>(value).ok())
                        && context.get("units").is_some() {
                        use aidash_domain::memory::{Content, Unit, Verification};
                        use reinhardt::query::{Alias, Expr, ExprTrait, Func, PostgresQueryBuilder, Query, QueryStatementBuilder};
                        let attempts = Query::select().column(Alias::new("id")).from(Alias::new("memory_model_attempts")).and_where(Expr::col("state").eq("pending")).to_owned();
                        let reservations: i64 = sqlx::query_scalar(&Query::select().expr(Func::count(Expr::col("attempt_id").into())).from(Alias::new("generation_usage")).and_where(Expr::col("attempt_id").in_subquery(attempts)).and_where(Expr::col("reported_tokens").is_null()).and_where(Expr::col("reserved_tokens").gt(0)).to_string(PostgresQueryBuilder)).fetch_one(&pool).await.unwrap();
                        assert_eq!(reservations, expected_origins.load(Ordering::SeqCst) as i64, "maintenance must retain every durable origin reservation before HTTP");
                        derived.fetch_add(1, Ordering::SeqCst);
                        if mode.load(Ordering::SeqCst) == 3 {
                            sqlx::query(&Query::update().table(Alias::new("generation_requests")).value(Alias::new("expires_at"),chrono::Utc::now()-chrono::Duration::seconds(1)).to_string(PostgresQueryBuilder)).execute(&pool).await.unwrap();
                        }

                        let units: Vec<Unit> = serde_json::from_value(context["units"].clone()).unwrap();
                        let mut result: Content = units[0].content.clone();
                        result.kind = serde_json::from_value(context["kind"].clone()).unwrap();
                        result.verification = Verification::Unverified;
                        result.evidence = units.iter().map(Unit::evidence).collect();
                        result.text = "Derived generated vehicle findings / 生成された車両の知見".into();
                        return Json(json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":serde_json::to_string(&result).unwrap()}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}));
                    }
                    calls.fetch_add(1, Ordering::SeqCst);
                    let mode = remember.load(Ordering::SeqCst);
                    let (name, arguments) = if mode > 0 {
                        let id = remembered_id(mode);
                        ("memory_mutate", serde_json::to_string(&json!({"changes":[{"operation":"add","id":id,"content":{"text":"Generated vehicle findings","kind":"experience","learning":"fact","verification":"unverified","occurred":null,"entities":[],"evidence":[],"links":[]}}]})).unwrap())
                    } else { ("workspace_observe", "{}".to_owned()) };
                    Json(json!({"choices":[{"index":0,"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"observe","type":"function","function":{"name":name,"arguments":arguments}}]}}],"usage":{"prompt_tokens":10,"completion_tokens":2}}))
                }
            }),
        );
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let endpoint = format!("http://{}", listener.local_addr().unwrap());
		let server = tokio::spawn(async move { axum::serve(listener, provider).await.unwrap() });
		let app = common::application(f.clone()).await;
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
		{
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
		for (kind, id, config) in [
			("reranker", "memory-reranker", json!({"provider":"rrf"})),
			(
				"tokenizer",
				"memory-tokenizer",
				json!({"provider":"utf8_upper_bound"}),
			),
			(
				"memory",
				"memory",
				json!({"engine":"hindsight_rust","policy":{
					"extraction":template["config"]["model"],"derivation":template["config"]["model"],"reflection":template["config"]["model"],
					"embedding":{"id":"embedding","version":"1.0.0"},"reranker":{"id":"memory-reranker","version":"1.0.0"},"tokenizer":{"id":"memory-tokenizer","version":"1.0.0"},
					"prices":{"extraction":{"input_per_million":0,"output_per_million":0},"derivation":{"input_per_million":0,"output_per_million":0},"reflection":{"input_per_million":0,"output_per_million":0},"embedding":{"input_per_million":0,"output_per_million":0},"reranker":{"input_per_million":0,"output_per_million":0}},
					"retention":{"unit_max_age_days":null,"candidate_days":7,"history_days":30,"history_versions":16,"model_result_days":7,"backup_days":7,"purge_after_seconds":60,"purge_batch":32,"max_unit_records":128,"max_model_operations":1024},
					"bounds":{"max_unit_bytes":8192,"max_input_bytes":8192,"max_units":16,"max_candidates":8,"max_entities":8,"max_evidence":8,"max_links":8,"max_graph_hops":3,"max_graph_visits":32,"max_results":4,"max_context_tokens":8192,"max_model_calls":4,"max_model_tokens":8192,"max_cost_micros":10000,"max_retries":2,"max_call_seconds":30},"semantic_link_min_similarity_millionths":700000,"learn_from_runs":false,"maintain_observations":maintenance,"refresh_mental_models":false
				}}),
			),
			(
				"source",
				"shared-memory",
				json!({"scope":"workspace","memory":{"id":"memory","version":"1.0.0"},"max_tokens":4096}),
			),
		] {
			let (status, response) = request(&app, &f.config.api_token, "POST", "/api/registry", json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id,"ja":id},"description":{"en":"Native memory fixture"},"config":config})).await;
			assert_eq!(status, 200, "{response}");
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
		}
		template["config"]["memory"] = json!({"id":"memory","version":"1.0.0"});
		template["config"]["allow_memory_write"] = json!(true);
		if !maintenance {
			template["config"]["sources"] = json!([{"id":"shared-memory","version":"1.0.0"}]);
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
		let (status, index) = request(&app,&f.config.api_token,"POST",&format!("/api/workspaces/{workspace}/semantic/index"),json!({"expected_revision":0,"spec":{"embedding":{"provider":"openai","endpoint":format!("{endpoint}/v1"),"credential_env":null,"model":"fixture-embedding","model_version":"1","dimensions":3},"vector":{"provider":"postgres","endpoint":"local","credential_env":null},"enabled":true,"auto_context":true,"max_sources":64,"max_results":10,"max_result_tokens":4096,"max_input_bytes":8192}})).await;
		assert_eq!(status, 200, "{index}");

		if maintenance {
			assert_eq!(request(&app,&token,"POST",&format!("/api/workspaces/{workspace}/semantic/entries"),json!({"key":"reference","expected_revision":0,"source":{"kind":"memory","text":"A car carries passengers."},"metadata":{}})).await.0,200);
		} else {
			let bank = json!({"home":f.store.node_id,"tenant":"acme","workspace":workspace,"participant":null});
			let (status,response)=request(&app,&f.config.api_token,"POST",&format!("/api/workspaces/{workspace}/memory/operate"),json!({"operation_id":Uuid::now_v7(),"provider":{"id":"memory","version":"1.0.0"},"bank":bank,"action":{"action":"configure_bank","expected_revision":0}})).await;
			assert_eq!(status, 200, "{response}");
			let (status,response)=request(&app,&f.config.api_token,"POST",&format!("/api/workspaces/{workspace}/memory/units/mutate"),json!({"operation_id":Uuid::now_v7(),"provider":{"id":"memory","version":"1.0.0"},"bank":bank,"changes":[{"operation":"add","id":Uuid::now_v7(),"content":{"text":"A car carries passengers.","kind":"world","learning":"fact","verification":"unverified","occurred":null,"entities":[],"evidence":[],"links":[]}}]})).await;
			assert_eq!(status, 200, "{response}");
		}

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
		aidash_server::generation::provision::reconcile(&f)
			.await
			.unwrap();
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
			derivations,
			maintenance_origins,
			embedding_started,
			server,
			memory_recovery_directory,
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
				if matches!(run.phase().as_str(), "COMPLETED" | "FAILED" | "CANCELLED")
					|| run.control == aidash_server::domain::RunControl::Paused
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
				.map(|run| {
					let phase = run.phase();
					(run.agent_id, phase, run.control)
				})
				.collect();
			panic!(
				"generated execution did not settle in 20s: {states:?}; embedding calls={}, inference calls={}",
				self.embeddings.load(Ordering::SeqCst),
				self.inference.load(Ordering::SeqCst)
			);
		}
	}
	async fn remember(&self) -> Uuid {
		self.remember_source(1).await
	}
	async fn remember_source(&self, ordinal: usize) -> Uuid {
		self.remember.store(ordinal, Ordering::SeqCst);
		// Match drive's coverage allowance: native context and generated-origin
		// checks span multiple worker turns and real database/provider boundaries.
		tokio::time::timeout(std::time::Duration::from_secs(20), async {
			let worker = Harness {
				federation: self.f.clone(),
			};
			loop {
				worker.worker_once().await.unwrap();
				let entry: Option<Uuid> = sqlx::query_scalar(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::Alias::new("id")),
						))
						.from(reinhardt::query::Alias::new("memory_units"))
						.and_where(reinhardt::query::Expr::col("kind").eq("experience"))
						.and_where(
							reinhardt::query::Expr::col("id")
								.eq(reinhardt::query::Expr::value(remembered_id(ordinal))),
						)
						.limit(1)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.fetch_optional(self.f.store.pool.driver())
				.await
				.unwrap();
				if let Some(entry) = entry {
					break entry;
				}
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		})
		.await
		.unwrap_or_else(|_| panic!("memory_mutate did not persist source {ordinal} within 20s; embedding calls={}, inference calls={}", self.embeddings.load(Ordering::SeqCst), self.inference.load(Ordering::SeqCst)))
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
		.fetch_all(self.f.store.pool.driver())
		.await
		.unwrap();
		for (collection, config) in collections {
			semantic::backend::delete_collection(
				&self.f.store,
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

fn remembered_id(ordinal: usize) -> Uuid {
	match ordinal {
		1 => Uuid::parse_str("fed375fe-6450-4768-8fa7-cc1af172b333").unwrap(),
		2 => Uuid::parse_str("fed375fe-6450-4768-8fa7-cc1af172b334").unwrap(),
		_ => panic!("unknown generated memory fixture ordinal"),
	}
}

#[rstest::rstest]
#[tokio::test]
async fn generated_semantic_context_without_embedding_approval_never_calls_provider(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let fixture = Fixture::new(&_test_environment, None, Some(2)).await;
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
	let fixture = Fixture::new(&_test_environment, Some(2), Some(2)).await;
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
	aidash_server::generation::provision::reconcile(&fixture.f)
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
	let fixture = Fixture::new(&_test_environment, Some(1), None).await;
	fixture.drive().await;
	let reserved: i64 = sqlx::query_scalar(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::SimpleExpr::from(
				reinhardt::query::Expr::col(reinhardt::query::Alias::new("reserved_tokens")),
			))
			.from(reinhardt::query::Alias::new("generation_embedding_usage"))
			.limit(1)
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_one(fixture.f.store.pool.driver())
	.await
	.unwrap();
	let usage = fixture.usage().await;
	assert_eq!(usage["embedding_calls"], 1);
	assert_eq!(usage["used_tokens"], reserved + 12);
	assert_eq!(fixture.embeddings.load(Ordering::SeqCst), 2);
	fixture.dispose().await;
	let fixture = Fixture::new(&_test_environment, Some(2), Some(2)).await;
	sqlx::query(
		&reinhardt::query::Query::update()
			.table(reinhardt::query::Alias::new("generation_requests"))
			.value_expr(
				reinhardt::query::Alias::new("expires_at"),
				reinhardt::query::Expr::cust("CLOCK_TIMESTAMP() - INTERVAL '1 SECOND'"),
			)
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.execute(fixture.f.store.pool.driver())
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
		let fixture = Fixture::new(&_test_environment, Some(2), Some(2)).await;
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
			let authorization = aidash_server::authorization::Authorization {
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
			assert_eq!(status, 409, "{index}");
			assert_eq!(
				index["error"],
				"Workspace index embedding differs from a pinned memory bank policy"
			);
			let (_, unchanged) = request(
				&fixture.app,
				&fixture.f.config.api_token,
				"GET",
				&format!("/api/workspaces/{}/semantic/index", fixture.workspace),
				Value::Null,
			)
			.await;
			assert_eq!(unchanged["revision"], fixture.index["revision"]);
			assert_eq!(unchanged["spec"], fixture.index["spec"]);
			fixture.dispose().await;
			continue;
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
	let fixture = Fixture::new(&_test_environment, Some(2), Some(2)).await;
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
	let (reserved, reported): (i64, Option<i64>) = {
		let query_bind_1 = entry;
		sqlx::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("reserved_tokens")),
				))
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("reported_tokens")),
				))
				.from(reinhardt::query::Alias::new("generation_embedding_usage"))
				.and_where(reinhardt::query::SimpleExpr::CustomWithExpr(
					"(purpose = 'index' AND entry_id = ?)".to_owned(),
					vec![reinhardt::query::Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(fixture.f.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(reported, None);
	assert_eq!(fixture.usage().await["used_tokens"], reserved + 14);
	fixture.response_mode.store(0, Ordering::SeqCst);
	{
		let query_bind_1 = entry;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("semantic_entries"))
				.value_expr(
					reinhardt::query::Alias::new("next_attempt"),
					reinhardt::query::Expr::cust("CLOCK_TIMESTAMP()"),
				)
				.and_where(reinhardt::query::SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![reinhardt::query::Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(fixture.f.store.pool.driver())
		.await
	}
	.unwrap();
	let recovered = fixture.f.store.isolated_pool().await.unwrap();
	semantic::worker::sweep(&recovered).await.unwrap();
	recovered.pool.close().await;
	recovered.control_pool.close().await;
	let (state, attempts): (String, i32) = {
		let query_bind_1 = entry;
		sqlx::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("state")),
				))
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("attempts")),
				))
				.from(reinhardt::query::Alias::new("semantic_entries"))
				.and_where(reinhardt::query::SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![reinhardt::query::Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(fixture.f.store.pool.driver())
		.await
	}
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
		let fixture = Fixture::new(&_test_environment, Some(2), Some(2)).await;
		let entry = fixture.remember().await;
		if expired {
			sqlx::query(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("generation_requests"))
					.value_expr(
						reinhardt::query::Alias::new("expires_at"),
						reinhardt::query::Expr::cust("CLOCK_TIMESTAMP() - INTERVAL '1 SECOND'"),
					)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(fixture.f.store.pool.driver())
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
		let state: String = {
			let query_bind_1 = entry;
			sqlx::query_scalar(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col(reinhardt::query::Alias::new("state")),
					))
					.from(reinhardt::query::Alias::new("semantic_entries"))
					.and_where(reinhardt::query::SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![reinhardt::query::Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(fixture.f.store.pool.driver())
			.await
		}
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
	let fixture = Fixture::new(&_test_environment, Some(2), Some(1_000_000)).await;
	fixture.drive().await;
	let reserved: i64 = sqlx::query_scalar(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::SimpleExpr::from(
				reinhardt::query::Expr::col(reinhardt::query::Alias::new("reserved_tokens")),
			))
			.from(reinhardt::query::Alias::new("generation_embedding_usage"))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_one(fixture.f.store.pool.driver())
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
		let mut fixture = Fixture::new(&_test_environment, Some(1), Some(2)).await;
		let parent = fixture.job.clone();
		let parent_run = fixture.f.store.runs().await.unwrap().remove(0);
		fixture
			.f
			.store
			.control(
				parent_run.id,
				aidash_server::domain::RunControlAction::Pause,
			)
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
			aidash_server::domain::qualified_agent(
				&fixture.f.config.node_id,
				&parent_run.agent_id,
				&parent_run.agent_version,
			),
		];
		{
			let query_bind_1 = task;
			let query_bind_2 = parent_run.id;
			let query_bind_3 = chain;
			sqlx::query(
				&reinhardt::query::Query::insert()
					.into_table(reinhardt::query::Alias::new("authorization_task_origins"))
					.columns([
						reinhardt::query::Alias::new("task_id"),
						reinhardt::query::Alias::new("source_run_id"),
						reinhardt::query::Alias::new("tenant"),
						reinhardt::query::Alias::new("root_subject"),
						reinhardt::query::Alias::new("subject_chain"),
					])
					.from_subquery(
						reinhardt::query::Query::select()
							.expr(reinhardt::query::SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![reinhardt::query::Expr::value(query_bind_1.to_owned()).into()],
							))
							.expr(reinhardt::query::SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![reinhardt::query::Expr::value(query_bind_2.to_owned()).into()],
							))
							.expr(reinhardt::query::Expr::cust("'acme'"))
							.expr(reinhardt::query::Expr::cust("'alice'"))
							.expr(reinhardt::query::SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![reinhardt::query::SimpleExpr::CustomWithExpr(
									format!(
										"ARRAY[{}]::text[]",
										std::iter::repeat_n("?", query_bind_3.len())
											.collect::<Vec<_>>()
											.join(",")
									),
									query_bind_3
										.iter()
										.map(|value| {
											reinhardt::query::Expr::value(value.clone()).into()
										})
										.collect(),
								)],
							))
							.to_owned(),
					)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(fixture.f.store.pool.driver())
			.await
		}
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
		aidash_server::generation::provision::reconcile(&fixture.f)
			.await
			.unwrap();
		if case == "token_exhausted" {
			{
				let query_bind_1 = parent["id"].as_str().unwrap().parse::<Uuid>().unwrap();
				sqlx::query(
					&reinhardt::query::Query::update()
						.table(reinhardt::query::Alias::new("generation_budgets"))
						.value_expr(
							reinhardt::query::Alias::new("used_tokens"),
							reinhardt::query::Expr::cust("token_limit - 1"),
						)
						.and_where(reinhardt::query::SimpleExpr::CustomWithExpr(
							"(request_id = ?)".to_owned(),
							vec![reinhardt::query::Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.execute(fixture.f.store.pool.driver())
				.await
			}
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

#[rstest::rstest]
#[tokio::test]
async fn killed_embedding_worker_retains_uncertain_usage_and_restart_reserves_a_new_attempt(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let fixture = Fixture::new(&_test_environment, Some(2), Some(2)).await;
	fixture.response_mode.store(2, Ordering::SeqCst);
	let mut worker = WorkerProcess::start_with_memory(
		&fixture.f,
		&fixture.url,
		&fixture.schema,
		fixture.memory_recovery_directory.path(),
	);
	let reached = tokio::time::timeout(
		std::time::Duration::from_secs(45),
		fixture.embedding_started.notified(),
	)
	.await;
	let states = fixture.f.store.runs().await.unwrap();
	let activity: Vec<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
		&reinhardt::query::Query::select()
			.columns(
				["state", "wait_event_type", "wait_event", "application_name"]
					.map(reinhardt::query::Alias::new),
			)
			.from(reinhardt::query::Alias::new("pg_stat_activity"))
			.and_where(
				reinhardt::query::Expr::col("datname")
					.eq(reinhardt::query::Expr::cust("CURRENT_DATABASE()")),
			)
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_all(fixture.f.store.pool.driver())
	.await
	.unwrap();
	assert!(
		reached.is_ok(),
		"embedding provider not reached; worker status {:?}; phases={:?}; database activity={activity:?}; log={}",
		worker.process.try_wait(),
		states
			.iter()
			.map(|run| (run.phase().as_str(), run.step))
			.collect::<Vec<_>>(),
		worker.log()
	);
	drop(worker); // Real SIGKILL, before the provider returns usage or a vector.
	let reserved: i64 = sqlx::query_scalar(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::SimpleExpr::from(
				reinhardt::query::Expr::col(reinhardt::query::Alias::new("reserved_tokens")),
			))
			.from(reinhardt::query::Alias::new("generation_embedding_usage"))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_one(fixture.f.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(fixture.usage().await["used_tokens"], reserved);
	assert_eq!(fixture.usage().await["embedding_calls"], 1);
	fixture.response_mode.store(0, Ordering::SeqCst);
	sqlx::query(
		&reinhardt::query::Query::update()
			.table(reinhardt::query::Alias::new("runs"))
			.value_expr(
				reinhardt::query::Alias::new("lease_until"),
				reinhardt::query::Expr::cust("CLOCK_TIMESTAMP() - INTERVAL '1 SECOND'"),
			)
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.execute(fixture.f.store.pool.driver())
	.await
	.unwrap();
	let worker = WorkerProcess::start_with_memory(
		&fixture.f,
		&fixture.url,
		&fixture.schema,
		fixture.memory_recovery_directory.path(),
	);
	tokio::time::timeout(std::time::Duration::from_secs(20), async {
		loop {
			let run = fixture.f.store.runs().await.unwrap().remove(0);
			if run.phase().as_str() == "FAILED" {
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
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::Expr::cust("COUNT(*)"))
			.expr(reinhardt::query::Expr::cust("COUNT(reported_tokens)"))
			.from(reinhardt::query::Alias::new("generation_embedding_usage"))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_one(fixture.f.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(attempts, (2, 1));
	fixture.dispose().await;
}

use reinhardt::query::QueryStatementBuilder as _;

#[rstest::rstest]
#[tokio::test]
async fn native_maintenance_retains_generated_origin_budgets_and_rechecks_expiry(
	#[future(awt)]
	#[from(test_environment)]
	environment: std::sync::Arc<TestEnvironment>,
) {
	use reinhardt::query::{
		Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
	};
	for mode in ["current", "expired", "exhausted", "expires_during_call"] {
		let fixture = Fixture::new_with_maintenance(&environment, Some(4), Some(2), true).await;
		let _source = fixture.remember().await;
		if mode == "expires_during_call" {
			fixture.response_mode.store(3, Ordering::SeqCst);
		}
		let before = fixture.usage().await["used_tokens"].as_i64().unwrap();
		if mode == "expired" {
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_requests"))
					.value(
						Alias::new("expires_at"),
						chrono::Utc::now() - chrono::Duration::seconds(1),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(fixture.f.store.pool.driver())
			.await
			.unwrap();
		}
		if mode == "exhausted" {
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_budgets"))
					.value_expr(Alias::new("used_tokens"), Expr::col("token_limit"))
					.to_string(PostgresQueryBuilder),
			)
			.execute(fixture.f.store.pool.driver())
			.await
			.unwrap();
		}
		tokio::time::timeout(
			std::time::Duration::from_secs(10),
			semantic::worker::sweep(&fixture.f.store),
		)
		.await
		.expect("maintenance must not deadlock its origin authority")
		.unwrap();
		assert_eq!(
			fixture.derivations.load(Ordering::SeqCst),
			usize::from(matches!(mode, "current" | "expires_during_call")),
			"{mode}"
		);
		let state: String = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("state"))
				.from(Alias::new("memory_engine_jobs"))
				.and_where(Expr::col("kind").eq("observation"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(fixture.f.store.pool.driver())
		.await
		.unwrap();
		if mode == "current" {
			assert_eq!(state, "complete");
			let after = fixture.usage().await["used_tokens"].as_i64().unwrap();
			assert_eq!(
				after,
				before + 4,
				"source embedding and derivation each settle two reported tokens"
			);
			semantic::worker::sweep(&fixture.f.store).await.unwrap();
			assert_eq!(
				fixture.derivations.load(Ordering::SeqCst),
				1,
				"completed work is idempotent"
			);
		} else {
			assert!(
				matches!(state.as_str(), "blocked" | "failed"),
				"{mode}: {state}"
			);
		}
		fixture.dispose().await;
	}
}

#[rstest::rstest]
#[tokio::test]
async fn native_shared_synthesis_preserves_published_origins_and_unions_nested_budgets(
	#[future(awt)]
	#[from(test_environment)]
	environment: std::sync::Arc<TestEnvironment>,
) {
	use aidash_domain::{memory::*, registry::EntityRef};
	use aidash_server::{
		authorization::identity::Actor, semantic::services::native_memory as memory,
	};
	use reinhardt::query::{
		Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
	};
	for mode in ["current", "expired", "exhausted"] {
		let mut fixture = Fixture::new(&environment, Some(8), Some(2)).await;
		let first = fixture.remember().await;
		let parent = fixture.f.store.runs().await.unwrap().remove(0);
		let parent_request: Uuid = fixture.job["id"].as_str().unwrap().parse().unwrap();
		fixture
			.f
			.store
			.control(parent.id, aidash_server::domain::RunControlAction::Pause)
			.await
			.unwrap();
		let (status, task) = request(&fixture.app, &fixture.token, "POST",
			&format!("/api/workspaces/{}/tasks", fixture.workspace),
			json!({"title":"Nested memory author / 入れ子の記憶作成者","description":"Keep origin lineage","requirements":{"capability":"semantic.research"}})).await;
		assert_eq!(status, 200, "{task}");
		let task: Uuid = task["id"].as_str().unwrap().parse().unwrap();
		let chain = [
			"alice".to_owned(),
			aidash_server::domain::qualified_agent(
				&fixture.f.store.node_id,
				&parent.agent_id,
				&parent.agent_version,
			),
		];
		// Trusted task_create origin; its worker transport has separate coverage.
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("authorization_task_origins"))
				.columns(
					[
						"task_id",
						"source_run_id",
						"tenant",
						"root_subject",
						"subject_chain",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(Expr::value(task))
						.expr(Expr::value(parent.id))
						.expr(Expr::value("acme"))
						.expr(Expr::value("alice"))
						.expr(reinhardt::query::SimpleExpr::CustomWithExpr(
							"ARRAY[?,?]::text[]".into(),
							chain
								.iter()
								.cloned()
								.map(|value| Expr::value(value).into())
								.collect(),
						))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(fixture.f.store.pool.driver())
		.await
		.unwrap();
		let (status, child) = request(
			&fixture.app,
			&fixture.token,
			"POST",
			&format!("/api/generation/acme/tasks/{task}/assign"),
			json!({"policy_id":"semantic","reason":"nested memory"}),
		)
		.await;
		assert_eq!(status, 200, "{child}");
		fixture.job = child["generation"].clone();
		let child_request: Uuid = fixture.job["id"].as_str().unwrap().parse().unwrap();
		aidash_server::generation::provision::reconcile(&fixture.f)
			.await
			.unwrap();
		let second = fixture.remember_source(2).await;
		let child_run = fixture
			.f
			.store
			.runs()
			.await
			.unwrap()
			.into_iter()
			.find(|run| run.id != parent.id)
			.unwrap();
		fixture
			.f
			.store
			.control(child_run.id, aidash_server::domain::RunControlAction::Pause)
			.await
			.unwrap();
		let provider = EntityRef {
			id: "memory".into(),
			version: "1.0.0".into(),
		};
		let shared = Bank {
			home: fixture.f.store.node_id.clone(),
			tenant: "acme".into(),
			workspace: fixture.workspace,
			participant: None,
		};
		let mut published = vec![];
		for (source, run) in [(first, parent.id), (second, child_run.id)] {
			let participant: Uuid = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("participant_id"))
					.from(Alias::new("memory_run_bindings"))
					.and_where(Expr::col("run_id").eq(Expr::value(run)))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(fixture.f.store.pool.driver())
			.await
			.unwrap();
			let private = Bank {
				participant: Some(participant),
				..shared.clone()
			};
			let unit = memory::list(
				&fixture.f.store,
				&Actor::Operator,
				memory::ReadBank {
					provider: provider.clone(),
					bank: private,
				},
			)
			.await
			.unwrap()
			.into_iter()
			.find(|unit| unit.id == source)
			.unwrap();
			let operation = Uuid::now_v7();
			let destination = Uuid::now_v7();
			let (status, result) = request(
				&fixture.app,
				&fixture.token,
				"POST",
				&format!("/api/workspaces/{}/memory/operate", fixture.workspace),
				serde_json::to_value(memory::Operation {
					operation_id: operation,
					provider: provider.clone(),
					bank: shared.clone(),
					action: memory::Action::Publish {
						source: unit.evidence(),
						mutation: Mutation {
							operation_id: operation,
							provider: provider.clone(),
							bank: shared.clone(),
							changes: vec![Change::Add {
								id: destination,
								content: unit.content,
							}],
						},
					},
				})
				.unwrap(),
			)
			.await;
			assert_eq!(status, 200, "{mode}: {result}");
			let memory::Outcome::Units(mut result) = serde_json::from_value(result).unwrap() else {
				panic!("publication")
			};
			let result = result.remove(0);
			let origins: Vec<Uuid> = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("run_id"))
					.from(Alias::new("memory_unit_run_origins"))
					.and_where(Expr::col("unit_id").eq(Expr::value(destination)))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(fixture.f.store.pool.driver())
			.await
			.unwrap();
			assert_eq!(
				origins,
				vec![run],
				"explicit publication preserves its generated origin"
			);
			published.push(result);
		}
		let budget_query = || {
			Query::select()
				.columns(["request_id", "used_tokens"].map(Alias::new))
				.from(Alias::new("generation_budgets"))
				.order_by(Alias::new("request_id"), reinhardt::query::Order::Asc)
				.to_string(PostgresQueryBuilder)
		};
		if mode == "expired" {
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_requests"))
					.value(
						Alias::new("expires_at"),
						chrono::Utc::now() - chrono::Duration::seconds(1),
					)
					.and_where(Expr::col("id").eq(Expr::value(child_request)))
					.to_string(PostgresQueryBuilder),
			)
			.execute(fixture.f.store.pool.driver())
			.await
			.unwrap();
		}
		if mode == "exhausted" {
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_budgets"))
					.value_expr(Alias::new("used_tokens"), Expr::col("token_limit"))
					.and_where(Expr::col("request_id").eq(Expr::value(child_request)))
					.to_string(PostgresQueryBuilder),
			)
			.execute(fixture.f.store.pool.driver())
			.await
			.unwrap();
		}
		let before: Vec<(Uuid, i64)> = sqlx::query_as(&budget_query())
			.fetch_all(fixture.f.store.pool.driver())
			.await
			.unwrap();
		fixture.maintenance_origins.store(2, Ordering::SeqCst);
		let operation = Uuid::now_v7();
		let target = Uuid::now_v7();
		let input = memory::Operation {
			operation_id: operation,
			provider: provider.clone(),
			bank: shared.clone(),
			action: memory::Action::Derive {
				mutation: Mutation {
					operation_id: operation,
					provider: provider.clone(),
					bank: shared.clone(),
					changes: vec![Change::Add {
						id: target,
						content: Content {
							mental_model: None,
							text: String::new(),
							kind: Kind::Observation,
							learning: Learning::Fact,
							verification: Verification::Unverified,
							occurred: None,
							entities: vec![],
							evidence: vec![],
							links: vec![],
						},
					}],
				},
				kind: Kind::Observation,
				sources: published.iter().map(Unit::evidence).collect(),
			},
		};
		let (status, result) = request(
			&fixture.app,
			&fixture.token,
			"POST",
			&format!("/api/workspaces/{}/memory/operate", fixture.workspace),
			serde_json::to_value(input).unwrap(),
		)
		.await;
		let after: Vec<(Uuid, i64)> = sqlx::query_as(&budget_query())
			.fetch_all(fixture.f.store.pool.driver())
			.await
			.unwrap();
		if mode == "current" {
			assert_eq!(status, 200, "{result}");
			let memory::Outcome::Units(result) = serde_json::from_value(result).unwrap() else {
				panic!("synthesis")
			};
			assert_eq!(result.len(), 1);
			assert_eq!(
				result[0].content.evidence,
				published.iter().map(Unit::evidence).collect::<Vec<_>>()
			);
			let mut origins: Vec<Uuid> = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("run_id"))
					.from(Alias::new("memory_unit_run_origins"))
					.and_where(Expr::col("unit_id").eq(Expr::value(target)))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(fixture.f.store.pool.driver())
			.await
			.unwrap();
			origins.sort_unstable();
			let mut expected = vec![parent.id, child_run.id];
			expected.sort_unstable();
			assert_eq!(origins, expected);
			assert_eq!(before.len(), 2);
			assert_eq!(after.len(), 2);
			for ((id, before), (after_id, after)) in before.iter().zip(&after) {
				assert_eq!(id, after_id);
				assert_eq!(*after - *before, 2, "the common ancestor is charged once");
			}
			let attempts = Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("memory_model_attempts"))
				.and_where(Expr::col("operation_id").eq(Expr::value(operation)))
				.to_owned();
			let mut charged: Vec<Uuid> = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("request_id"))
					.from(Alias::new("generation_usage"))
					.and_where(Expr::col("attempt_id").in_subquery(attempts))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(fixture.f.store.pool.driver())
			.await
			.unwrap();
			charged.sort_unstable();
			let mut expected = vec![parent_request, child_request];
			expected.sort_unstable();
			assert_eq!(charged, expected);
		} else {
			assert_ne!(status, 200, "{mode}: {result}");
			assert_eq!(
				fixture.derivations.load(Ordering::SeqCst),
				0,
				"no provider call after a contributing origin loses allowance"
			);
			assert_eq!(
				before, after,
				"a failed late reservation never partially charges an ancestor"
			);
			let units: Vec<Uuid> = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("memory_units"))
					.and_where(Expr::col("id").eq(Expr::value(target)))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(fixture.f.store.pool.driver())
			.await
			.unwrap();
			assert!(units.is_empty());
		}
		fixture.dispose().await;
	}
}
