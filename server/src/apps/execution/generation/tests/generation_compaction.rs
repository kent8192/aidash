#[path = "../../tests/support/worker_process.rs"]
mod worker_process;
use worker_process::WorkerProcess;
#[path = "../../tests/support/legacy.rs"]
mod common;
use aidash_server::{federation::Federation, harness::Harness};
use axum::{Json, Router, routing::post};
use common::*;
use common::{TestEnvironment, test_environment};
use serde_json::{Value, json};
use std::sync::{
	Arc,
	atomic::{AtomicUsize, Ordering},
};

async fn policy(f: &Federation, app: &common::TestApplication, endpoint: &str) -> (String, Value) {
	let (_, token, _) = bootstrap(f, app, endpoint).await;
	let compactor = json!({"id":"jev","version":"1.0.0","kind":"compactor","name":{"en":"Approved Jev"},"description":{"en":"Local fixture"},"capabilities":[],"languages":[],"tags":[],"skills":[],"schema":{},"config":{"provider":"typesafe-system-one","endpoint":format!("{endpoint}/systemone"),"model":"fixture-jev","credential_env":"AIDASH_SECRET_TEST_PEER","max_request_bytes":200000,"max_questions":200,"max_response_bytes":16000}});
	let (status, body) =
		request(app, &f.config.api_token, "POST", "/api/registry", compactor).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(
		app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"jev","version":"1.0.0"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (_, mut template) = request(
		app,
		&f.config.api_token,
		"GET",
		"/api/registry/research/1.0.0",
		Value::Null,
	)
	.await;
	template["id"] = json!("template");
	template["capabilities"] = json!(["compact.research"]);
	let spec = json!({"enabled":true,"template":template,"permissions":{"roles":[],"groups":[],"attributes":{}},"approval_required":false,"limits":{"max_agents":4,"max_concurrent":4,"max_depth":2,"token_budget":2000000,"tokens_per_agent":400000,"lifetime_seconds":3600},"compaction":{"provider":{"id":"jev","version":"1.0.0"},"calls_per_agent":2,"call_budget":4}});
	let (status, body) = request(
		app,
		&f.config.api_token,
		"POST",
		"/api/generation/acme/policies/research",
		json!({"expected_revision":0,"spec":spec}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	(token, spec)
}
async fn assign(app: &common::TestApplication, token: &str) -> (u16, Value) {
	let (_, ws) = request(
		app,
		token,
		"POST",
		"/api/workspaces",
		json!({"title":"Compaction","goal":"Long research"}),
	)
	.await;
	let (_, task) = request(app, token, "POST", &format!("/api/workspaces/{}/tasks", ws["id"].as_str().unwrap()), json!({"title":"Research","description":"Long history","requirements":{"capability":"compact.research"}})).await;
	request(
		app,
		token,
		"POST",
		&format!(
			"/api/generation/acme/tasks/{}/assign",
			task["id"].as_str().unwrap()
		),
		json!({"policy_id":"research","reason":"missing specialist"}),
	)
	.await
}
async fn long_history(f: &Federation, job: &Value) -> aidash_server::domain::Run {
	aidash_server::generation::provision::reconcile(f)
		.await
		.unwrap();
	Harness {
		federation: f.clone(),
	}
	.worker_once()
	.await
	.unwrap();
	let run = f
		.store
		.runs()
		.await
		.unwrap()
		.into_iter()
		.find(|r| r.agent_id == job["agent_id"].as_str().unwrap())
		.unwrap();
	seed_history(f, &run).await;
	run
}
async fn seed_history(f: &Federation, run: &aidash_server::domain::Run) {
	let mut history = vec![
		json!({"kind":"tool","call":{"id":"first","name":"read","arguments":{}},"result":"keep first"}),
		json!({"kind":"tool","call":{"id":"obsolete","name":"read","arguments":{}},"result":"old".repeat(50000)}),
	];
	for i in 0..6 {
		history.push(json!({"kind":"tool","call":{"id":format!("recent-{i}"),"name":"read","arguments":{}},"result":"recent"}));
	}
	{
		let query_bind_1 = run.id;
		let mut context = serde_json::to_value(&run.context).unwrap();
		context["history"] = json!(history);
		let query_bind_2 = context;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("phase"),
					reinhardt::query::Expr::cust("'THINKING'"),
				)
				.value_expr(
					reinhardt::query::Alias::new("pending"),
					Expr::value(common::pending(aidash_server::domain::RunState::Thinking(
						Default::default(),
					))),
				)
				.value_expr(
					reinhardt::query::Alias::new("context"),
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
}

#[rstest::rstest]
#[tokio::test]
async fn generated_agent_budget_includes_the_models_full_output_limit(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = common::application(f.clone()).await;
	let (_, mut spec) = policy(&f, &app, "http://127.0.0.1:9").await;
	let model = json!({
		"id":"large-output-model","version":"1.0.0","kind":"model",
		"name":{"en":"Large output model"},"description":{"en":"Budget fixture"},
		"capabilities":[],"languages":["en"],"tags":[],"skills":[],
		"schema":{"type":"object"},
		"config":{"provider":"openrouter","model_id":"google/gemini-3.8-flash",
			"endpoint":"https://openrouter.ai/api/v1","credential_env":null,
			"context_window":1048576,"max_output_tokens":65536,
			"modalities":["text"],"cost":{}}
	});
	let (status, body) = request(&app, &f.config.api_token, "POST", "/api/registry", model).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"large-output-model","version":"1.0.0"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	spec["template"]["config"]["model"] = json!({"id":"large-output-model","version":"1.0.0"});
	spec["limits"]["tokens_per_agent"] = json!(1_114_111);
	let (status, _) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/generation/acme/policies/research",
		json!({"expected_revision":1,"spec":spec}),
	)
	.await;
	assert_ne!(status, 200);
	spec["limits"]["tokens_per_agent"] = json!(1_114_112);
	let (status, body) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/generation/acme/policies/research",
		json!({"expected_revision":1,"spec":spec}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn approved_compaction_is_pinned_bounded_and_accounted_before_http(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let pool = f.store.pool.driver().clone();
	let calls = Arc::new(AtomicUsize::new(0));
	let seen = calls.clone();
	let app = Router::new().route("/systemone", post(move |Json(body): Json<Value>| {
        let seen = seen.clone(); let pool = pool.clone(); async move {
            seen.fetch_add(1, Ordering::SeqCst);
            let attempts: i64 = sqlx::query_scalar(&reinhardt::query::Query::select().expr(reinhardt::query::Expr::cust("COUNT(*)")).from(reinhardt::query::Alias::new("generation_compaction_usage")).to_string(reinhardt::query::PostgresQueryBuilder)).fetch_one(&pool).await.unwrap();
            assert_eq!(attempts, 1, "reservation must be committed before HTTP");
            assert_eq!(body["model"], "fixture-jev");
            let answers: serde_json::Map<_,_> = body["questions"].as_object().unwrap().keys().map(|key| (key.clone(), json!({"noul":0.0}))).collect();
            Json(json!({"answers":answers}))
        }
    })).route("/v1/chat/completions", post(|| async { Json(json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Compacted result"}}],"usage":{"prompt_tokens":100,"completion_tokens":10}})) }));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
	let app = common::application(f.clone()).await;
	let (token, mut spec) = policy(&f, &app, &endpoint).await;
	let (status, assigned) = assign(&app, &token).await;
	assert_eq!(status, 200, "{assigned}");
	let job = &assigned["generation"];
	// Later policy edits cannot replace the request's approved provider contract.
	spec["compaction"] = Value::Null;
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/generation/acme/policies/research",
			json!({"expected_revision":1,"spec":spec})
		)
		.await
		.0,
		200
	);
	let run = long_history(&f, job).await;
	let worker = Harness {
		federation: f.clone(),
	};
	for _ in 0..8 {
		worker.worker_once().await.unwrap();
	}
	let completed = f.store.run(run.id).await.unwrap();
	assert_eq!(completed.phase().as_str(), "COMPLETED", "{completed:?}");
	assert_eq!(json!(completed.context)["compactions"], 1);
	assert_eq!(calls.load(Ordering::SeqCst), 1);
	let (_, usage) = request(
		&app,
		&token,
		"GET",
		&format!(
			"/api/generation/acme/requests/{}/usage",
			job["id"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	assert_eq!(usage["compaction_call_limit"], 2);
	assert_eq!(usage["compaction_calls"], 1);
	aidash_server::generation::provision::reconcile(&f)
		.await
		.unwrap();
	let (_, policies) = request(
		&app,
		&token,
		"GET",
		"/api/generation/acme/policies",
		Value::Null,
	)
	.await;
	assert_eq!(policies[0]["allocated_compaction_calls"], 1);
	server.abort();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn compaction_total_budget_is_atomic_and_unused_calls_release_once(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = common::application(f.clone()).await;
	let (token, _) = policy(&f, &app, "http://127.0.0.1:9").await;
	let (first, second) = tokio::join!(assign(&app, &token), assign(&app, &token));
	assert_eq!(first.0, 200);
	assert_eq!(second.0, 200);
	assert_eq!(assign(&app, &token).await.0, 409);
	let path = format!(
		"/api/generation/acme/requests/{}/control",
		first.1["generation"]["id"].as_str().unwrap()
	);
	for _ in 0..2 {
		assert_eq!(
			request(
				&app,
				&token,
				"POST",
				&path,
				json!({"action":"stop","reason":"release unused allowance"})
			)
			.await
			.0,
			200
		);
	}
	let (_, policies) = request(
		&app,
		&token,
		"GET",
		"/api/generation/acme/policies",
		Value::Null,
	)
	.await;
	assert_eq!(policies[0]["allocated_compaction_calls"], 2);
	assert_eq!(assign(&app, &token).await.0, 200);
	assert_eq!(assign(&app, &token).await.0, 409);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn failed_compaction_attempts_remain_charged_and_exhaustion_prevents_http(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let calls = Arc::new(AtomicUsize::new(0));
	let seen = calls.clone();
	let server = Router::new().route(
		"/systemone",
		post(move || {
			let seen = seen.clone();
			async move {
				seen.fetch_add(1, Ordering::SeqCst);
				http::StatusCode::TOO_MANY_REQUESTS
			}
		}),
	);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
	let (f, url, schema) = setup(&_test_environment).await;
	let app = common::application(f.clone()).await;
	let (token, _) = policy(&f, &app, &endpoint).await;
	let (_, assignment) = assign(&app, &token).await;
	let run = long_history(&f, &assignment["generation"]).await;
	for expected in 1..=3 {
		// A fresh worker instance observes committed usage from earlier attempts.
		Harness {
			federation: f.clone(),
		}
		.worker_once()
		.await
		.unwrap();
		assert_eq!(calls.load(Ordering::SeqCst), expected.min(2));
		{
			let query_bind_1 = run.id;
			sqlx::query(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("runs"))
					.value_expr(
						reinhardt::query::Alias::new("pending"),
						reinhardt::query::Expr::cust(
							"jsonb_set(pending, '{recovery,retry}', 'null'::jsonb)",
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
	}
	let current = f.store.run(run.id).await.unwrap();
	assert!(
		current
			.error
			.unwrap()
			.contains("compaction call budget exhausted")
	);
	let counts: (i64, i64) = sqlx::query_as(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::Expr::cust("COUNT(*)"))
			.expr(reinhardt::query::Expr::cust("COUNT(DISTINCT attempt_id)"))
			.from(reinhardt::query::Alias::new("generation_compaction_usage"))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_one(f.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(counts, (2, 2));
	let tokens: i64 = sqlx::query_scalar(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::SimpleExpr::from(
				reinhardt::query::Expr::col(reinhardt::query::Alias::new("used_tokens")),
			))
			.from(reinhardt::query::Alias::new("generation_budgets"))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_one(f.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(tokens, 0, "no inference started");
	server.abort();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn compaction_denial_and_catalog_revocation_prevent_disclosure(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	for revoke_catalog in [false, true] {
		let (f, url, schema) = setup(&_test_environment).await;
		let app = common::application(f.clone()).await;
		let (token, _) = policy(&f, &app, "http://127.0.0.1:9").await;
		let (_, assignment) = assign(&app, &token).await;
		let run = long_history(&f, &assignment["generation"]).await;
		if revoke_catalog {
			assert_eq!(
				request(
					&app,
					&f.config.api_token,
					"POST",
					"/api/authorization/acme/catalog",
					json!({"entry":{"id":"jev","version":"1.0.0"},"expected_revision":1,"enabled":false})
				)
				.await
				.0,
				200
			);
		} else {
			let snapshot = aidash_server::authorization::Authorization {
				pool: f.store.pool.clone(),
			}
			.snapshot("acme")
			.await
			.unwrap();
			let mut bundle = json!(snapshot.bundle);
			bundle["policies"].as_array_mut().unwrap().push(json!({"id":"deny-compaction","effect":"deny","subjects":{"ids":[aidash_server::domain::qualified_agent(&f.config.node_id,&run.agent_id,&run.agent_version)]},"actions":["compaction.invoke"],"resources":{"kinds":["compactor"]}}));
			assert_eq!(
				request(
					&app,
					&f.config.api_token,
					"POST",
					"/api/authorization/acme",
					json!({"expected_revision":snapshot.revision,"bundle":bundle})
				)
				.await
				.0,
				200
			);
		}
		Harness {
			federation: f.clone(),
		}
		.worker_once()
		.await
		.unwrap();
		assert_eq!(
			f.store.run(run.id).await.unwrap().control.as_str(),
			"PAUSED"
		);
		let count: i64 = sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust("COUNT(*)"))
				.from(reinhardt::query::Alias::new("generation_compaction_usage"))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
		.unwrap();
		assert_eq!(count, 0);
		cleanup(f, &url, &schema).await;
	}
}

#[rstest::rstest]
#[tokio::test]
async fn process_restart_preserves_provisioning_and_uncertain_compaction_charge(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let first_model = Arc::new(tokio::sync::Notify::new());
	let first_compaction = Arc::new(tokio::sync::Notify::new());
	let model_calls = Arc::new(AtomicUsize::new(0));
	let compaction_calls = Arc::new(AtomicUsize::new(0));
	let server = Router::new()
        .route("/v1/chat/completions", post({
            let started = first_model.clone(); let calls = model_calls.clone();
            move || { let started=started.clone(); let calls=calls.clone(); async move {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    started.notify_one(); std::future::pending::<()>().await;
                }
                Json(json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Recovered result"}}],"usage":{"prompt_tokens":100,"completion_tokens":10}}))
            }}
        }))
        .route("/systemone", post({
            let started=first_compaction.clone(); let calls=compaction_calls.clone(); let pool=f.store.pool.driver().clone();
            move |Json(body):Json<Value>| { let started=started.clone(); let calls=calls.clone(); let pool=pool.clone(); async move {
                let number=calls.fetch_add(1, Ordering::SeqCst)+1;
                let committed:i64=sqlx::query_scalar(&reinhardt::query::Query::select().expr(reinhardt::query::Expr::cust("COUNT(*)")).from(reinhardt::query::Alias::new("generation_compaction_usage")).to_string(reinhardt::query::PostgresQueryBuilder)).fetch_one(&pool).await.unwrap();
                assert_eq!(committed,number as i64);
                if number == 1 { started.notify_one(); std::future::pending::<()>().await; }
                let answers:serde_json::Map<_,_>=body["questions"].as_object().unwrap().keys().map(|k|(k.clone(),json!({"noul":0.0}))).collect();
                Json(json!({"answers":answers}))
            }}
        }));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
	let app = common::application(f.clone()).await;
	let (token, _) = policy(&f, &app, &endpoint).await;
	let (_, assigned) = assign(&app, &token).await;
	let job = &assigned["generation"];
	assert_eq!(job["status"], "QUEUED");
	assert!(f.store.runs().await.unwrap().is_empty());
	let mut worker = WorkerProcess::start(&f, &url, &schema);
	let reached =
		tokio::time::timeout(std::time::Duration::from_secs(20), first_model.notified()).await;
	assert!(
		reached.is_ok(),
		"worker status {:?}, run states {:?}, log {}",
		worker.process.try_wait(),
		f.store
			.runs()
			.await
			.unwrap()
			.iter()
			.map(|r| (r.phase(), &r.control, &r.error))
			.collect::<Vec<_>>(),
		worker.log()
	);
	drop(worker); // SIGKILL: no graceful settlement or application cleanup.
	let runs = f.store.runs().await.unwrap();
	assert_eq!(runs.len(), 1);
	let run = &runs[0];
	seed_history(&f, run).await;
	{
		let query_bind_1 = run.id;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("lease_until"),
					reinhardt::query::Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 SECOND'"),
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
	let worker = WorkerProcess::start(&f, &url, &schema);
	tokio::time::timeout(
		std::time::Duration::from_secs(20),
		first_compaction.notified(),
	)
	.await
	.unwrap();
	drop(worker);
	{
		let query_bind_1 = run.id;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("lease_until"),
					reinhardt::query::Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 SECOND'"),
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
	let worker = WorkerProcess::start(&f, &url, &schema);
	tokio::time::timeout(std::time::Duration::from_secs(20), async {
		loop {
			if f.store.run(run.id).await.unwrap().phase().as_str() == "COMPLETED" {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(50)).await;
		}
	})
	.await
	.unwrap();
	drop(worker);
	assert_eq!(compaction_calls.load(Ordering::SeqCst), 2);
	assert_eq!(model_calls.load(Ordering::SeqCst), 2);
	assert_eq!(f.store.runs().await.unwrap().len(), 1);
	let (_, usage) = request(
		&app,
		&token,
		"GET",
		&format!(
			"/api/generation/acme/requests/{}/usage",
			job["id"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	assert_eq!(usage["compaction_calls"], 2);
	assert_eq!(usage["used_tokens"], 132206); // Uncertain inference + bounded successful inference.
	let entries: i64 = sqlx::query_scalar(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::Expr::cust("COUNT(*)"))
			.from(reinhardt::query::Alias::new("registry"))
			.and_where(reinhardt::query::Expr::cust("id LIKE 'generated-%'"))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_one(f.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(entries, 1);
	server.abort();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn nested_generation_intersects_compaction_approval_and_charges_both_ancestors(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	for approved in [true, false] {
		let (f, url, schema) = setup(&_test_environment).await;
		let calls = Arc::new(AtomicUsize::new(0));
		let seen = calls.clone();
		let pool = f.store.pool.driver().clone();
		let server=Router::new().route("/systemone",post(move |Json(body):Json<Value>| {
            let seen=seen.clone(); let pool=pool.clone(); async move {
                seen.fetch_add(1,Ordering::SeqCst);
                let charges:i64=sqlx::query_scalar(&reinhardt::query::Query::select().expr(reinhardt::query::Expr::cust("COUNT(*)")).from(reinhardt::query::Alias::new("generation_compaction_usage")).to_string(reinhardt::query::PostgresQueryBuilder)).fetch_one(&pool).await.unwrap();
                assert_eq!(charges,2,"both ancestor reservations precede disclosure");
                let answers:serde_json::Map<_,_>=body["questions"].as_object().unwrap().keys().map(|k|(k.clone(),json!({"noul":0.0}))).collect();
                Json(json!({"answers":answers}))
            }
        })).route("/v1/chat/completions",post(||async{Json(json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Nested result"}}],"usage":{"prompt_tokens":100,"completion_tokens":10}}))}));
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let endpoint = format!("http://{}", listener.local_addr().unwrap());
		let server = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
		let app = common::application(f.clone()).await;
		let (token, mut spec) = policy(&f, &app, &endpoint).await;
		let (_, parent) = assign(&app, &token).await;
		aidash_server::generation::provision::reconcile(&f)
			.await
			.unwrap();
		let parent_run = f.store.runs().await.unwrap().remove(0);
		{
			let query_bind_1 = parent_run.id;
			sqlx::query(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("runs"))
					.value_expr(
						reinhardt::query::Alias::new("control"),
						reinhardt::query::Expr::cust("'PAUSED'"),
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
		if !approved {
			spec["compaction"] = Value::Null;
			assert_eq!(
				request(
					&app,
					&f.config.api_token,
					"POST",
					"/api/generation/acme/policies/research",
					json!({"expected_revision":1,"spec":spec})
				)
				.await
				.0,
				200
			);
		}
		let (_,child)=request(&app,&token,"POST",&format!("/api/workspaces/{}/tasks",parent_run.workspace_id),json!({"title":"Child","description":"Inherited compaction budget","requirements":{"capability":"compact.research"}})).await;
		let child_id: uuid::Uuid = child["id"].as_str().unwrap().parse().unwrap();
		// Seed the trusted origin written by task_create. Its transport behavior
		// is tested separately; this case targets the runtime budget intersection.
		let chain = vec![
			"alice".to_owned(),
			aidash_server::domain::qualified_agent(
				&f.config.node_id,
				&parent_run.agent_id,
				&parent_run.agent_version,
			),
		];
		{
			let query_bind_1 = child_id;
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
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							))
							.expr(reinhardt::query::Expr::cust("'acme'"))
							.expr(reinhardt::query::Expr::cust("'alice'"))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![SimpleExpr::CustomWithExpr(
									format!(
										"ARRAY[{}]::text[]",
										std::iter::repeat_n("?", query_bind_3.len())
											.collect::<Vec<_>>()
											.join(",")
									),
									query_bind_3
										.iter()
										.map(|value| Expr::value(value.clone()).into())
										.collect(),
								)],
							))
							.to_owned(),
					)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(f.store.pool.driver())
			.await
		}
		.unwrap();
		let (status, child) = request(
			&app,
			&token,
			"POST",
			&format!("/api/generation/acme/tasks/{child_id}/assign"),
			json!({"policy_id":"research","reason":"nested work"}),
		)
		.await;
		assert_eq!(status, 200, "{child}");
		let run = long_history(&f, &child["generation"]).await;
		let worker = Harness {
			federation: f.clone(),
		};
		let current = tokio::time::timeout(std::time::Duration::from_secs(5), async {
			loop {
				worker.worker_once().await.unwrap();
				let current = f.store.run(run.id).await.unwrap();
				if matches!(
					current.phase().as_str(),
					"COMPLETED" | "FAILED" | "CANCELLED"
				) {
					break current;
				}
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		})
		.await
		.unwrap();
		assert_eq!(
			current.phase().as_str(),
			if approved { "COMPLETED" } else { "FAILED" },
			"phase={}, error={:?}, pending={}",
			current.phase().as_str(),
			current.error,
			json!(current.state)["data"]
		);
		assert_eq!(calls.load(Ordering::SeqCst), usize::from(approved));
		for job in [&parent["generation"], &child["generation"]] {
			let (_, usage) = request(
				&app,
				&token,
				"GET",
				&format!(
					"/api/generation/acme/requests/{}/usage",
					job["id"].as_str().unwrap()
				),
				Value::Null,
			)
			.await;
			assert_eq!(usage["compaction_calls"], i64::from(approved));
			assert_eq!(usage["used_tokens"], if approved { 110 } else { 0 });
		}
		server.abort();
		cleanup(f, &url, &schema).await;
	}
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::SimpleExpr;

use reinhardt::query::Expr;
