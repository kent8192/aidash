mod common;
use aidash::api;
use common::*;
use common::{TestEnvironment, test_environment};
use serde_json::{Value, json};

async fn definition(app: &axum::Router, token: &str) -> Value {
	let (_, mut template) = request(
		app,
		token,
		"GET",
		"/api/registry/research/1.0.0",
		Value::Null,
	)
	.await;
	template["id"] = json!("research-template");
	template["capabilities"] = json!(["special.research"]);
	json!({"enabled":true,"template":template,"permissions":{"roles":[],"groups":[],"attributes":{"team":"research"}},"approval_required":true,"limits":{"max_agents":4,"max_concurrent":2,"max_depth":2,"token_budget":800000,"tokens_per_agent":200000,"lifetime_seconds":3600}})
}

#[rstest::rstest]
#[tokio::test]
async fn generation_policy_is_revisioned_and_requests_reserve_deduplicated_quota(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let spec = definition(&app, &f.config.api_token).await;
	let (status, created) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/generation/acme/policies/research",
		json!({"expected_revision":0,"spec":spec}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	assert_eq!(created["revision"], 1);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/generation/acme/policies/research",
			json!({"expected_revision":0,"spec":spec})
		)
		.await
		.0,
		409
	);
	let (_, workspace) = request(
		&app,
		&token,
		"POST",
		"/api/workspaces",
		json!({"title":"Missing agent","goal":"Specialist research"}),
	)
	.await;
	let (_,task)=request(&app,&token,"POST",&format!("/api/workspaces/{}/tasks",workspace["id"].as_str().unwrap()),json!({"title":"Specialist","description":"Find an approved specialist","requirements":{"capability":"special.research"}})).await;
	let path = format!(
		"/api/generation/acme/tasks/{}/assign",
		task["id"].as_str().unwrap()
	);
	let body = json!({"policy_id":"research","reason":"no approved specialist"});
	let (status, assignment) = request(&app, &token, "POST", &path, body.clone()).await;
	assert_eq!(status, 200, "{assignment}");
	assert_eq!(assignment["kind"], "generated");
	let generated = &assignment["generation"];
	assert_eq!(generated["status"], "PENDING_APPROVAL");
	assert_eq!(generated["definition"]["config"]["model"]["id"], "model");
	let repeated = request(&app, &token, "POST", &path, body).await;
	assert_eq!(repeated, (200, assignment));
	let (_, policies) = request(
		&app,
		&token,
		"GET",
		"/api/generation/acme/policies",
		Value::Null,
	)
	.await;
	assert_eq!(policies[0]["generated_count"], 1);
	assert_eq!(policies[0]["allocated_tokens"], 200000);
	assert!(
		f.store.runs().await.unwrap().is_empty(),
		"approval must precede activation"
	);
	cleanup(f, &url, &schema).await;
}

async fn missing_task(app: &axum::Router, token: &str) -> String {
	let (status, ws) = request(
		app,
		token,
		"POST",
		"/api/workspaces",
		json!({"title":"Generated work","goal":"Special research"}),
	)
	.await;
	assert_eq!(status, 200, "{ws}");
	let (status, task) = request(app, token, "POST", &format!("/api/workspaces/{}/tasks", ws["id"].as_str().unwrap()), json!({"title":"Specialist","description":"Find an approved specialist","requirements":{"capability":"special.research"}})).await;
	assert_eq!(status, 200, "{task}");
	task["id"].as_str().unwrap().to_owned()
}

#[rstest::rstest]
#[tokio::test]
async fn approval_activation_and_stop_are_atomic_and_audited(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let spec = definition(&app, &f.config.api_token).await;
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/generation/acme/policies/research",
			json!({"expected_revision":0,"spec":spec})
		)
		.await
		.0,
		200
	);
	let task = missing_task(&app, &token).await;
	let (_, assignment) = request(
		&app,
		&token,
		"POST",
		&format!("/api/generation/acme/tasks/{task}/assign"),
		json!({"policy_id":"research","reason":"missing specialist"}),
	)
	.await;
	let job = &assignment["generation"];
	let path = format!(
		"/api/generation/acme/requests/{}/control",
		job["id"].as_str().unwrap()
	);
	let (status, approved) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"action":"approve","reason":"reviewed definition"}),
	)
	.await;
	assert_eq!(status, 200, "{approved}");
	assert_eq!(approved["status"], "QUEUED");
	aidash::generation::provision::reconcile(&f).await.unwrap();
	let runs = f.store.runs().await.unwrap();
	assert_eq!(runs.len(), 1);
	assert_eq!(runs[0].agent_id, job["agent_id"].as_str().unwrap());
	let (status, replayed) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"action":"approve","reason":"reviewed definition"}),
	)
	.await;
	assert_eq!(status, 200);
	assert_eq!(replayed["status"], "ACTIVE");
	let grant: Vec<String> = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("subject_chain")),
			))
			.from(sea_orm::sea_query::Alias::new("authorization_execution"))
			.and_where(sea_orm::sea_query::Expr::cust("run_id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(runs[0].id)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(grant.len(), 2);
	assert_eq!(grant[0], "alice");
	let (status, stopped) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"action":"stop","reason":"no longer needed"}),
	)
	.await;
	assert_eq!(status, 200, "{stopped}");
	assert_eq!(stopped["status"], "STOPPED");
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&path,
			json!({"action":"stop","reason":"no longer needed"})
		)
		.await,
		(200, stopped)
	);
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&path,
			json!({"action":"approve","reason":"reopen"})
		)
		.await
		.0,
		409
	);
	let (_, policies) = request(
		&app,
		&token,
		"GET",
		"/api/generation/acme/policies",
		Value::Null,
	)
	.await;
	assert_eq!(policies[0]["allocated_tokens"], 0);
	assert_eq!(policies[0]["generated_count"], 1);
	let (status, history) = request(
		&app,
		&token,
		"GET",
		&path.replace("/control", "/history"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{history}");
	assert_eq!(history.as_array().unwrap().len(), 4);
	assert_eq!(history[3]["actor"], "alice");
	let subject = aidash::domain::qualified_agent(
		&f.config.node_id,
		job["agent_id"].as_str().unwrap(),
		"1.0.0",
	);
	let snapshot = aidash::authorization::Authorization {
		pool: f.store.pool.clone(),
	}
	.snapshot("acme")
	.await
	.unwrap();
	assert!(!snapshot.bundle.subjects[&subject].enabled);
	let worker = aidash::harness::Harness {
		federation: f.clone(),
	};
	worker.worker_once().await.unwrap();
	assert_eq!(f.store.run(runs[0].id).await.unwrap().phase, "CANCELLED");
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn generated_agent_completes_with_pinned_definition_and_refunds_unused_allowance(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	use axum::{Json, Router, routing::post};
	let server=Router::new().route("/v1/chat/completions",post(|Json(body):Json<Value>|async move {
        assert_eq!(body["model"],"fixture");
        Json(json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Specialist result"}}],"usage":{"prompt_tokens":120,"completion_tokens":20}}))
    }));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, &endpoint).await;
	let mut spec = definition(&app, &f.config.api_token).await;
	spec["approval_required"] = json!(false);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/generation/acme/policies/research",
			json!({"expected_revision":0,"spec":spec})
		)
		.await
		.0,
		200
	);
	let task = missing_task(&app, &token).await;
	let (status, assignment) = request(
		&app,
		&token,
		"POST",
		&format!("/api/generation/acme/tasks/{task}/assign"),
		json!({"policy_id":"research","reason":"specialist missing"}),
	)
	.await;
	assert_eq!(status, 200, "{assignment}");
	let job = &assignment["generation"];
	spec["template"]["config"]["instructions"] = json!("A later template revision");
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
	let (a, b) = tokio::join!(
		aidash::generation::provision::reconcile(&f),
		aidash::generation::provision::reconcile(&f)
	);
	a.unwrap();
	b.unwrap();
	let runs = f.store.runs().await.unwrap();
	assert_eq!(runs.len(), 1);
	let entry = f
		.registry
		.get(&runs[0].agent_id, &runs[0].agent_version)
		.await
		.unwrap();
	assert_eq!(entry.config["instructions"], "Test approved work");
	let worker = aidash::harness::Harness {
		federation: f.clone(),
	};
	for _ in 0..8 {
		worker.worker_once().await.unwrap();
	}
	assert_eq!(f.store.run(runs[0].id).await.unwrap().phase, "COMPLETED");
	assert_eq!(
		f.store.task(task.parse().unwrap()).await.unwrap().status,
		"COMPLETED"
	);
	aidash::generation::provision::reconcile(&f).await.unwrap();
	aidash::generation::provision::reconcile(&f).await.unwrap();
	let (_, policies) = request(
		&app,
		&token,
		"GET",
		"/api/generation/acme/policies",
		Value::Null,
	)
	.await;
	assert_eq!(policies[0]["allocated_tokens"], 140);
	let (_, jobs) = request(
		&app,
		&token,
		"GET",
		"/api/generation/acme/requests",
		Value::Null,
	)
	.await;
	assert_eq!(jobs[0]["status"], "COMPLETED");
	assert_eq!(jobs[0]["policy_revision"], 1);
	let (status, pinned) = request(
		&app,
		&token,
		"GET",
		&format!(
			"/api/generation/acme/requests/{}/spec",
			job["id"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200);
	assert_eq!(
		pinned["template"]["config"]["instructions"],
		"Test approved work"
	);
	assert_eq!(pinned["permissions"]["attributes"]["team"], "research");
	let (status, usage) = request(
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
	assert_eq!(status, 200);
	assert_eq!(
		usage,
		json!({"token_limit":200000,"used_tokens":140,"inference_attempts":1,"compaction_call_limit":0,"compaction_calls":0,"embedding_call_limit":0,"embedding_calls":0})
	);
	assert_eq!(jobs[0]["id"], job["id"]);
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!(
				"/api/generation/acme/requests/{}/control",
				job["id"].as_str().unwrap()
			),
			json!({"action":"delete","reason":"finished"})
		)
		.await
		.0,
		200
	);
	server.abort();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn concurrent_requests_obey_quota_and_denial_releases_it_once(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let mut spec = definition(&app, &f.config.api_token).await;
	spec["limits"]["max_concurrent"] = json!(1);
	spec["limits"]["token_budget"] = json!(200000);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/generation/acme/policies/research",
			json!({"expected_revision":0,"spec":spec})
		)
		.await
		.0,
		200
	);
	let a = missing_task(&app, &token).await;
	let b = missing_task(&app, &token).await;
	let pa = format!("/api/generation/acme/tasks/{a}/assign");
	let pb = format!("/api/generation/acme/tasks/{b}/assign");
	let body = json!({"policy_id":"research","reason":"specialist missing"});
	let (ra, rb) = tokio::join!(
		request(&app, &token, "POST", &pa, body.clone()),
		request(&app, &token, "POST", &pb, body.clone())
	);
	let (accepted, retry) = match (ra.0, rb.0) {
		(200, 409) => (ra.1, pb),
		(409, 200) => (rb.1, pa),
		_ => panic!("{ra:?} {rb:?}"),
	};
	let control = format!(
		"/api/generation/acme/requests/{}/control",
		accepted["generation"]["id"].as_str().unwrap()
	);
	let denied = request(
		&app,
		&token,
		"POST",
		&control,
		json!({"action":"deny","reason":"unnecessary"}),
	)
	.await;
	assert_eq!(denied.0, 200, "{denied:?}");
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&control,
			json!({"action":"deny","reason":"unnecessary"})
		)
		.await,
		denied
	);
	assert_eq!(request(&app, &token, "POST", &retry, body).await.0, 200);
	let (_, policies) = request(
		&app,
		&token,
		"GET",
		"/api/generation/acme/policies",
		Value::Null,
	)
	.await;
	assert_eq!(policies[0]["generated_count"], 2);
	assert_eq!(policies[0]["allocated_tokens"], 200000);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn agent_created_tasks_keep_generation_depth_when_requested_by_root(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	use axum::{Json, Router, routing::post};
	let server=Router::new().route("/v1/chat/completions",post(||async {
        Json(json!({"choices":[{"index":0,"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"create-child","type":"function","function":{"name":"task_create","arguments":json!({"title":"Child specialist","description":"Nested generation","requirements":{"capability":"special.research"}}).to_string()}}]}}],"usage":{"prompt_tokens":10,"completion_tokens":10}}))
    }));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, &endpoint).await;
	let mut spec = definition(&app, &f.config.api_token).await;
	spec["approval_required"] = json!(false);
	spec["limits"]["max_depth"] = json!(1);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/generation/acme/policies/research",
			json!({"expected_revision":0,"spec":spec})
		)
		.await
		.0,
		200
	);
	let task = missing_task(&app, &token).await;
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/generation/acme/tasks/{task}/assign"),
			json!({"policy_id":"research","reason":"parent"})
		)
		.await
		.0,
		200
	);
	aidash::generation::provision::reconcile(&f).await.unwrap();
	let worker = aidash::harness::Harness {
		federation: f.clone(),
	};
	for _ in 0..3 {
		worker.worker_once().await.unwrap();
	}
	let children: Vec<aidash::domain::Task> = sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
			))
			.from(sea_orm::sea_query::Alias::new("tasks"))
			.and_where(sea_orm::sea_query::Expr::cust("parent_id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(task.parse::<uuid::Uuid>().unwrap())
	.fetch_all(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(children.len(), 1);
	let origin: Vec<String> = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("subject_chain")),
			))
			.from(sea_orm::sea_query::Alias::new("authorization_task_origins"))
			.and_where(sea_orm::sea_query::Expr::cust("task_id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(children[0].id)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(origin.len(), 2);
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&format!("/api/generation/acme/tasks/{}/assign", children[0].id),
		json!({"policy_id":"research","reason":"nested"}),
	)
	.await;
	assert_eq!(status, 409, "{body}");
	assert!(body.to_string().contains("depth"));
	let (_, policies) = request(
		&app,
		&token,
		"GET",
		"/api/generation/acme/policies",
		Value::Null,
	)
	.await;
	assert_eq!(policies[0]["generated_count"], 1);
	server.abort();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn revoked_requester_cannot_activate_and_failed_admission_leaves_no_agent(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let mut spec = definition(&app, &f.config.api_token).await;
	spec["approval_required"] = json!(false);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/generation/acme/policies/research",
			json!({"expected_revision":0,"spec":spec})
		)
		.await
		.0,
		200
	);
	let task = missing_task(&app, &token).await;
	let (_, assignment) = request(
		&app,
		&token,
		"POST",
		&format!("/api/generation/acme/tasks/{task}/assign"),
		json!({"policy_id":"research","reason":"request before revocation"}),
	)
	.await;
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("authorization_credentials"))
			.value(
				sea_orm::sea_query::Alias::new("revoked_at"),
				sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("subject = 'alice'"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.execute(&f.store.pool)
	.await
	.unwrap();
	aidash::generation::provision::reconcile(&f).await.unwrap();
	let (_, jobs) = request(
		&app,
		&f.config.api_token,
		"GET",
		"/api/generation/acme/requests",
		Value::Null,
	)
	.await;
	assert_eq!(jobs[0]["status"], "FAILED");
	assert!(f.store.runs().await.unwrap().is_empty());
	assert!(
		f.registry
			.get(
				assignment["generation"]["agent_id"].as_str().unwrap(),
				"1.0.0"
			)
			.await
			.is_err()
	);
	let (_, policies) = request(
		&app,
		&f.config.api_token,
		"GET",
		"/api/generation/acme/policies",
		Value::Null,
	)
	.await;
	assert_eq!(policies[0]["allocated_tokens"], 0);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn missing_usage_keeps_reservation_and_stops_before_another_model_call(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	use axum::{Json, Router, routing::post};
	use std::sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	};
	let calls = Arc::new(AtomicUsize::new(0));
	let count = calls.clone();
	let server=Router::new().route("/v1/chat/completions",post(move ||{let count=count.clone();async move {
        count.fetch_add(1,Ordering::SeqCst);
        Json(json!({"choices":[{"index":0,"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"observe","type":"function","function":{"name":"workspace_observe","arguments":"{}"}}]}}],"usage":{"completion_tokens":1}}))
    }}));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, &endpoint).await;
	let mut spec = definition(&app, &f.config.api_token).await;
	spec["approval_required"] = json!(false);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/generation/acme/policies/research",
			json!({"expected_revision":0,"spec":spec})
		)
		.await
		.0,
		200
	);
	let task = missing_task(&app, &token).await;
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/generation/acme/tasks/{task}/assign"),
			json!({"policy_id":"research","reason":"bounded work"})
		)
		.await
		.0,
		200
	);
	aidash::generation::provision::reconcile(&f).await.unwrap();
	let worker = aidash::harness::Harness {
		federation: f.clone(),
	};
	// Failure delivery is scheduled by wake_at and compared with PostgreSQL's
	// clock. Wait for that durable transition rather than counting fast polls.
	let run = tokio::time::timeout(std::time::Duration::from_secs(5), async {
		loop {
			worker.worker_once().await.unwrap();
			let run = f.store.runs().await.unwrap().remove(0);
			if run.phase == "FAILED" {
				break run;
			}
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	})
	.await
	.unwrap();
	assert_eq!(run.phase, "FAILED");
	assert_eq!(calls.load(Ordering::SeqCst), 1);
	aidash::generation::provision::reconcile(&f).await.unwrap();
	let (_, policies) = request(
		&app,
		&token,
		"GET",
		"/api/generation/acme/policies",
		Value::Null,
	)
	.await;
	assert_eq!(policies[0]["allocated_tokens"], 132096);
	server.abort();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn worker_can_request_nested_generation_without_dropping_parent_authority(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	use axum::{Json, Router, routing::post};
	let server=Router::new().route("/v1/chat/completions",post(|Json(body):Json<Value>|async move {
        let context:Value=serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
        let child=context["current"]["workspace"]["tasks"].as_array().unwrap().iter().find(|t|t["parent_id"]==context["current"]["task"]["id"]);
        let (name,arguments)=if let Some(child)=child {("task_assign",json!({"task_id":child["id"],"policy_id":"research","reason":"nested specialist"}))}else{("task_create",json!({"title":"Child specialist","description":"Nested generation","requirements":{"capability":"special.research"}}))};
        Json(json!({"choices":[{"index":0,"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":name,"type":"function","function":{"name":name,"arguments":arguments.to_string()}}]}}],"usage":{"prompt_tokens":10,"completion_tokens":10}}))
    }));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, &endpoint).await;
	let mut spec = definition(&app, &f.config.api_token).await;
	spec["approval_required"] = json!(false);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/generation/acme/policies/research",
			json!({"expected_revision":0,"spec":spec})
		)
		.await
		.0,
		200
	);
	let task = missing_task(&app, &token).await;
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/generation/acme/tasks/{task}/assign"),
			json!({"policy_id":"research","reason":"parent"})
		)
		.await
		.0,
		200
	);
	aidash::generation::provision::reconcile(&f).await.unwrap();
	let worker = aidash::harness::Harness {
		federation: f.clone(),
	};
	tokio::time::timeout(std::time::Duration::from_secs(10), async {
		for _ in 0..6 {
			worker.worker_once().await.unwrap();
		}
	})
	.await
	.expect("nested assignment must not deadlock its authority lease");
	let (_, jobs) = request(
		&app,
		&token,
		"GET",
		"/api/generation/acme/requests",
		Value::Null,
	)
	.await;
	assert_eq!(jobs.as_array().unwrap().len(), 2, "{jobs}");
	let child = jobs
		.as_array()
		.unwrap()
		.iter()
		.find(|j| j["depth"] == 2)
		.unwrap();
	assert_eq!(child["status"], "QUEUED");
	assert_eq!(child["subject_chain"].as_array().unwrap().len(), 2);
	aidash::generation::provision::reconcile(&f).await.unwrap();
	let runs = f.store.runs().await.unwrap();
	assert_eq!(runs.len(), 2);
	server.abort();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn generation_reads_and_events_respect_denial_and_tenant_boundaries(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (mut policy, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let spec = definition(&app, &f.config.api_token).await;
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/generation/acme/policies/research",
			json!({"expected_revision":0,"spec":spec})
		)
		.await
		.0,
		200
	);
	let task = missing_task(&app, &token).await;
	let path = format!("/api/generation/acme/tasks/{task}/assign");
	let body = json!({"policy_id":"research","reason":"specialist"});
	let (_, assignment) = request(&app, &token, "POST", &path, body.clone()).await;
	let job = &assignment["generation"];
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"private-generation","effect":"deny","subjects":{"ids":["alice"]},"actions":["generation.read"],"resources":{"kinds":["generation","generation_policy"]}}));
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
	assert_eq!(request(&app, &token, "POST", &path, body).await.0, 403);
	let (_, jobs) = request(
		&app,
		&token,
		"GET",
		"/api/generation/acme/requests",
		Value::Null,
	)
	.await;
	assert_eq!(jobs, json!([]));
	assert_eq!(
		request(
			&app,
			&token,
			"GET",
			&format!(
				"/api/generation/acme/requests/{}/spec",
				job["id"].as_str().unwrap()
			),
			Value::Null
		)
		.await
		.0,
		403
	);
	assert_eq!(
		request(
			&app,
			&token,
			"GET",
			&format!(
				"/api/generation/acme/requests/{}/usage",
				job["id"].as_str().unwrap()
			),
			Value::Null
		)
		.await
		.0,
		403
	);
	let (_, events) = request(&app, &token, "GET", "/api/events", Value::Null).await;
	assert!(
		events
			.as_array()
			.unwrap()
			.iter()
			.all(|e| !e["kind"].as_str().unwrap().starts_with("generation."))
	);
	assert_eq!(
		request(
			&app,
			&token,
			"GET",
			&format!(
				"/api/generation/acme/requests/{}/history",
				job["id"].as_str().unwrap()
			),
			Value::Null
		)
		.await
		.0,
		403
	);
	assert_eq!(
		request(
			&app,
			&token,
			"GET",
			"/api/generation/other/requests",
			Value::Null
		)
		.await
		.0,
		403
	);
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!(
				"/api/generation/other/requests/{}/control",
				job["id"].as_str().unwrap()
			),
			json!({"action":"approve","reason":"cross tenant"})
		)
		.await
		.0,
		403
	);
	let task2 = missing_task(&app, &token).await;
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/generation/acme/tasks/{task2}/assign"),
			json!({"policy_id":"research","reason":"denied read"})
		)
		.await
		.0,
		403
	);
	let (_, policies) = request(
		&app,
		&f.config.api_token,
		"GET",
		"/api/generation/acme/policies",
		Value::Null,
	)
	.await;
	assert_eq!(policies[0]["generated_count"], 1);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn expiration_cancels_generated_run_before_any_provider_call(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let mut spec = definition(&app, &f.config.api_token).await;
	spec["approval_required"] = json!(false);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/generation/acme/policies/research",
			json!({"expected_revision":0,"spec":spec})
		)
		.await
		.0,
		200
	);
	let task = missing_task(&app, &token).await;
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/generation/acme/tasks/{task}/assign"),
			json!({"policy_id":"research","reason":"expires"})
		)
		.await
		.0,
		200
	);
	aidash::generation::provision::reconcile(&f).await.unwrap();
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("generation_requests"))
			.value(
				sea_orm::sea_query::Alias::new("expires_at"),
				sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP() - INTERVAL '1 SECOND'"),
			)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.execute(&f.store.pool)
	.await
	.unwrap();
	let worker = aidash::harness::Harness {
		federation: f.clone(),
	};
	worker.worker_once().await.unwrap();
	let run = f.store.runs().await.unwrap().remove(0);
	assert_eq!(run.control, "PAUSED");
	assert_eq!(run.phase, "READY");
	aidash::generation::provision::reconcile(&f).await.unwrap();
	worker.worker_once().await.unwrap();
	assert_eq!(f.store.run(run.id).await.unwrap().phase, "CANCELLED");
	let (_, jobs) = request(
		&app,
		&token,
		"GET",
		"/api/generation/acme/requests",
		Value::Null,
	)
	.await;
	assert_eq!(jobs[0]["status"], "EXPIRED");
	let (_, policies) = request(
		&app,
		&token,
		"GET",
		"/api/generation/acme/policies",
		Value::Null,
	)
	.await;
	assert_eq!(policies[0]["allocated_tokens"], 0);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn stop_commits_during_inflight_inference_and_discards_its_result(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	use axum::{Json, Router, routing::post};
	use std::sync::Arc;
	use tokio::sync::Notify;
	let entered = Arc::new(Notify::new());
	let release = Arc::new(Notify::new());
	let started = entered.clone();
	let unblock = release.clone();
	let server=Router::new().route("/v1/chat/completions",post(move ||{let started=started.clone();let unblock=unblock.clone();async move {
        started.notify_one();unblock.notified().await;
        Json(json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Inflight result"}}],"usage":{"prompt_tokens":10,"completion_tokens":10}}))
    }}));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, &endpoint).await;
	let mut spec = definition(&app, &f.config.api_token).await;
	spec["approval_required"] = json!(false);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/generation/acme/policies/research",
			json!({"expected_revision":0,"spec":spec})
		)
		.await
		.0,
		200
	);
	let task = missing_task(&app, &token).await;
	let (_, assignment) = request(
		&app,
		&token,
		"POST",
		&format!("/api/generation/acme/tasks/{task}/assign"),
		json!({"policy_id":"research","reason":"controlled inference"}),
	)
	.await;
	aidash::generation::provision::reconcile(&f).await.unwrap();
	let worker = aidash::harness::Harness {
		federation: f.clone(),
	};
	worker.worker_once().await.unwrap();
	let running = tokio::spawn(async move { worker.worker_once().await.unwrap() });
	tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
		.await
		.unwrap();
	let used: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("used_tokens")),
			))
			.from(sea_orm::sea_query::Alias::new("generation_budgets"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(used, 132096, "reserve must commit before provider I/O");
	let stop_app = app.clone();
	let stop_token = token.clone();
	let path = format!(
		"/api/generation/acme/requests/{}/control",
		assignment["generation"]["id"].as_str().unwrap()
	);
	let stop = tokio::spawn(async move {
		request(
			&stop_app,
			&stop_token,
			"POST",
			&path,
			json!({"action":"stop","reason":"stop inference"}),
		)
		.await
	});
	let stopped = tokio::time::timeout(std::time::Duration::from_secs(5), stop)
		.await
		.expect("stop should not wait for the provider response")
		.unwrap();
	assert_eq!(stopped.0, 200, "{:#?}", stopped.1);
	release.notify_one();
	running.await.unwrap();
	let pending = f.store.runs().await.unwrap().remove(0);
	assert_eq!(pending.control, "CANCELLED");
	assert!(pending.pending.get("response").is_none());
	let worker = aidash::harness::Harness {
		federation: f.clone(),
	};
	worker.worker_once().await.unwrap();
	let run = f.store.runs().await.unwrap().remove(0);
	assert_eq!(run.phase, "CANCELLED");
	assert!(
		f.store
			.snapshot(run.workspace_id)
			.await
			.unwrap()
			.artifacts
			.is_empty()
	);
	let (_, policies) = request(
		&app,
		&token,
		"GET",
		"/api/generation/acme/policies",
		Value::Null,
	)
	.await;
	assert_eq!(policies[0]["allocated_tokens"], 20);
	server.abort();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn atomic_commit_discards_generated_output_but_settles_its_usage(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	use axum::{Json, Router, routing::post};
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	use std::sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	};
	use tokio::sync::Notify;

	let entered = Arc::new(Notify::new());
	let release = Arc::new(Notify::new());
	let calls = Arc::new(AtomicUsize::new(0));
	let provider = Router::new().route(
		"/v1/chat/completions",
		post({
			let entered = entered.clone();
			let release = release.clone();
			let calls = calls.clone();
			move || {
				let entered = entered.clone();
				let release = release.clone();
				let calls = calls.clone();
				async move {
					if calls.fetch_add(1, Ordering::SeqCst) == 0 {
						entered.notify_one();
						release.notified().await;
					}
					Json(json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Generated result"}}],"usage":{"prompt_tokens":10,"completion_tokens":2}}))
				}
			}
		}),
	);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, provider).await.unwrap() });
	let (mut f, url, schema) = setup(&_test_environment).await;
	f.config.lease_seconds = 300;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, &endpoint).await;
	let mut spec = definition(&app, &f.config.api_token).await;
	spec["approval_required"] = json!(false);
	let (status, response) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/generation/acme/policies/research",
		json!({"expected_revision":0,"spec":spec}),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	let task = missing_task(&app, &token).await;
	let (status, assignment) = request(
		&app,
		&token,
		"POST",
		&format!("/api/generation/acme/tasks/{task}/assign"),
		json!({"policy_id":"research","reason":"atomic inference"}),
	)
	.await;
	assert_eq!(status, 200, "{assignment}");
	aidash::generation::provision::reconcile(&f).await.unwrap();
	let worker = aidash::harness::Harness {
		federation: f.clone(),
	};
	assert!(worker.worker_once().await.unwrap());
	let run = f.store.runs().await.unwrap().remove(0);
	let waiting = tokio::spawn({
		let worker = worker.clone();
		async move { worker.worker_once().await }
	});
	tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
		.await
		.expect("provider must receive the first request");
	let mut transaction = f.store.control_pool.begin().await.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("atomic_gate"))
			.value(Alias::new("commit_epoch"), Expr::cust("commit_epoch + 1"))
			.cond_where(Expr::col(Alias::new("singleton")).eq(true))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *transaction)
	.await
	.unwrap();
	transaction.commit().await.unwrap();
	release.notify_one();
	assert!(
		tokio::time::timeout(std::time::Duration::from_secs(10), waiting)
			.await
			.expect("stale inference must finish")
			.unwrap()
			.unwrap()
	);
	let current = f.store.run(run.id).await.unwrap();
	assert_eq!(current.phase, "THINKING");
	assert!(current.pending.get("retry_at").is_some());
	assert!(current.pending.get("response").is_none());
	let (reserved, reported): (i64, Option<i64>) = sqlx::query_as(
		&Query::select()
			.columns(["reserved_tokens", "reported_tokens"].map(Alias::new))
			.from(Alias::new("generation_usage"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(reserved, 132096);
	assert_eq!(reported, Some(12));
	let used: i64 = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("used_tokens"))
			.from(Alias::new("generation_budgets"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(used, 12, "stale output must refund unused reserved tokens");
	tokio::time::sleep(std::time::Duration::from_secs(1)).await;
	assert!(worker.worker_once().await.unwrap());
	assert_eq!(calls.load(Ordering::SeqCst), 2);
	assert_eq!(f.store.run(run.id).await.unwrap().phase, "TOOL_CALL");
	server.abort();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn matching_ordinary_agent_is_reused_without_generation_or_quota(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (mut bundle, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	bundle["policies"][0]["resources"]["kinds"] = json!([
		"workspace",
		"generation_policy",
		"agent",
		"model",
		"tool",
		"run",
		"memory"
	]);
	bundle["policies"].as_array_mut().unwrap().push(json!({"id":"task-owner","effect":"allow","subjects":{"any":true},"actions":["task.read","task.execute","task.delegate"],"resources":{"kinds":["task"]},"condition":{"op":"eq","left":{"source":"resource","path":"/created_by"},"right":{"source":"literal","value":"alice"}}}));
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":bundle})
		)
		.await
		.0,
		200
	);
	let spec = definition(&app, &f.config.api_token).await;
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/generation/acme/policies/research",
			json!({"expected_revision":0,"spec":spec})
		)
		.await
		.0,
		200
	);
	let path = format!("/api/generation/acme/tasks/{task}/assign");
	let body = json!({"policy_id":"research","reason":"find ordinary executor"});
	let first = request(&app, &token, "POST", &path, body.clone()).await;
	assert_eq!(first.0, 200, "{first:?}");
	assert_eq!(first.1["kind"], "existing");
	assert_eq!(first.1["delegation"]["agent_id"], "research");
	assert_eq!(request(&app, &token, "POST", &path, body).await, first);
	let (_, jobs) = request(
		&app,
		&token,
		"GET",
		"/api/generation/acme/requests",
		Value::Null,
	)
	.await;
	assert_eq!(jobs, json!([]));
	let (_, policies) = request(
		&app,
		&token,
		"GET",
		"/api/generation/acme/policies",
		Value::Null,
	)
	.await;
	assert_eq!(policies[0]["generated_count"], 0);
	assert_eq!(policies[0]["allocated_tokens"], 0);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn disabling_an_existing_policy_remains_possible_after_component_revocation(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let mut spec = definition(&app, &f.config.api_token).await;
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			"/api/generation/acme/policies/research",
			json!({"expected_revision":0,"spec":spec})
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
			json!({"entry":{"id":"model","version":"1.0.0"},"expected_revision":1,"enabled":false})
		)
		.await
		.0,
		200
	);
	spec["enabled"] = json!(false);
	let (status, disabled) = request(
		&app,
		&token,
		"POST",
		"/api/generation/acme/policies/research",
		json!({"expected_revision":1,"spec":spec}),
	)
	.await;
	assert_eq!(status, 200, "{disabled}");
	assert_eq!(disabled["spec"]["enabled"], false);
	spec["enabled"] = json!(true);
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			"/api/generation/acme/policies/research",
			json!({"expected_revision":2,"spec":spec})
		)
		.await
		.0,
		400
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn count_concurrency_and_total_token_limits_are_independent(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	for limit in ["max_agents", "max_concurrent", "token_budget"] {
		let (f, url, schema) = setup(&_test_environment).await;
		let app = api::router(f.clone());
		let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
		let mut spec = definition(&app, &f.config.api_token).await;
		spec["limits"][limit] = json!(if limit == "token_budget" { 200000 } else { 1 });
		if limit == "max_agents" {
			spec["limits"]["max_concurrent"] = json!(1);
		}
		assert_eq!(
			request(
				&app,
				&f.config.api_token,
				"POST",
				"/api/generation/acme/policies/research",
				json!({"expected_revision":0,"spec":spec})
			)
			.await
			.0,
			200
		);
		let first = missing_task(&app, &token).await;
		let (status, first) = request(
			&app,
			&token,
			"POST",
			&format!("/api/generation/acme/tasks/{first}/assign"),
			json!({"policy_id":"research","reason":"first reservation"}),
		)
		.await;
		assert_eq!(status, 200, "{first}");
		if limit == "max_agents" {
			assert_eq!(
				request(
					&app,
					&token,
					"POST",
					&format!(
						"/api/generation/acme/requests/{}/control",
						first["generation"]["id"].as_str().unwrap()
					),
					json!({"action":"deny","reason":"lifetime count must remain"})
				)
				.await
				.0,
				200
			);
		}
		let second = missing_task(&app, &token).await;
		let path = format!("/api/generation/acme/tasks/{second}/assign");
		let body = json!({"policy_id":"research","reason":"second reservation"});
		assert_eq!(
			request(&app, &token, "POST", &path, body.clone()).await.0,
			409,
			"{limit} must independently reject admission"
		);
		let count: i64 = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
				.from(sea_orm::sea_query::Alias::new("generation_requests"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.fetch_one(&f.store.pool)
		.await
		.unwrap();
		assert_eq!(
			count, 1,
			"rejected request must leave no partial definition"
		);
		spec["limits"][limit] = json!(if limit == "token_budget" { 400000 } else { 2 });
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
		assert_eq!(
			request(&app, &token, "POST", &path, body).await.0,
			200,
			"raising only {limit} must admit the same task"
		);
		cleanup(f, &url, &schema).await;
	}
}

#[rstest::rstest]
#[tokio::test]
async fn generated_permission_attributes_deny_tools_without_losing_the_pending_call(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	use axum::{Json, Router, routing::post};
	use std::sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	};
	let effects = Arc::new(AtomicUsize::new(0));
	let count = effects.clone();
	let server=Router::new().route("/effect",post(move || {let count=count.clone();async move {count.fetch_add(1,Ordering::SeqCst);Json(json!({"saved":true}))}}))
        .route("/v1/chat/completions",post(|Json(body):Json<Value>|async move {
            let context:Value=serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
            let message=if context["history"].as_array().unwrap().iter().any(|e|e["kind"]=="tool") {json!({"role":"assistant","content":"Approved tool completed"})}
            else {json!({"role":"assistant","content":null,"tool_calls":[{"id":"generated-effect","type":"function","function":{"name":"plugin_0","arguments":"{}"}}]})};
            Json(json!({"choices":[{"index":0,"finish_reason":if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"},"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
        }));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (mut bundle, token, _) = bootstrap(&f, &app, &endpoint).await;
	bundle["policies"].as_array_mut().unwrap().push(json!({"id":"deny-research-tools","effect":"deny","subjects":{"kinds":["agent"]},"actions":["tool.invoke"],"resources":{"kinds":["tool"]},"condition":{"op":"eq","left":{"source":"subject","path":"/team"},"right":{"source":"literal","value":"research"}}}));
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":bundle})
		)
		.await
		.0,
		200
	);
	let mut spec = definition(&app, &f.config.api_token).await;
	spec["approval_required"] = json!(false);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/generation/acme/policies/research",
			json!({"expected_revision":0,"spec":spec})
		)
		.await
		.0,
		200
	);
	let task = missing_task(&app, &token).await;
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/generation/acme/tasks/{task}/assign"),
			json!({"policy_id":"research","reason":"restricted specialist"})
		)
		.await
		.0,
		200
	);
	aidash::generation::provision::reconcile(&f).await.unwrap();
	let worker = aidash::harness::Harness {
		federation: f.clone(),
	};
	for _ in 0..3 {
		worker.worker_once().await.unwrap();
	}
	let run = f.store.runs().await.unwrap().remove(0);
	assert_eq!(run.phase, "TOOL_CALL");
	assert_eq!(run.control, "PAUSED");
	assert_eq!(run.pending["cursor"], 0);
	assert_eq!(effects.load(Ordering::SeqCst), 0);
	let snapshot = aidash::authorization::Authorization {
		pool: f.store.pool.clone(),
	}
	.snapshot("acme")
	.await
	.unwrap();
	let mut bundle = json!(snapshot.bundle);
	bundle["policies"].as_array_mut().unwrap().pop();
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
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/runs/{}/control", run.id),
			json!({"action":"resume"})
		)
		.await
		.0,
		200
	);
	for _ in 0..8 {
		worker.worker_once().await.unwrap();
	}
	assert_eq!(f.store.run(run.id).await.unwrap().phase, "COMPLETED");
	assert_eq!(effects.load(Ordering::SeqCst), 1);
	server.abort();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn generation_visibility_paginates_and_cannot_override_later_event_ownership(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (mut policy, token, other_task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let spec = definition(&app, &f.config.api_token).await;
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/generation/acme/policies/research",
			json!({"expected_revision":0,"spec":spec})
		)
		.await
		.0,
		200
	);
	let task = missing_task(&app, &token).await;
	let (status, assignment) = request(
		&app,
		&token,
		"POST",
		&format!("/api/generation/acme/tasks/{task}/assign"),
		json!({"policy_id":"research","reason":"missing specialist"}),
	)
	.await;
	assert_eq!(status, 200, "{assignment}");
	let job = &assignment["generation"];
	let job_id: uuid::Uuid = job["id"].as_str().unwrap().parse().unwrap();
	// More than a page of newer denied requests must not hide the older visible one.
	{
		use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
		for n in 1..=201 {
			let child = uuid::Uuid::new_v4();
			let source = Query::select()
				.expr(Expr::cust("$2"))
				.column(Alias::new("workspace_id"))
				.expr(Expr::val("Hidden"))
				.expr(Expr::val("Fixture"))
				.expr(Expr::cust("'{}'::jsonb"))
				.expr(Expr::val("fixture"))
				.expr(Expr::val(format!("hidden-{n}")))
				.from(Alias::new("tasks"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.to_owned();
			sqlx::query(
				&Query::insert()
					.into_table(Alias::new("tasks"))
					.columns(
						[
							"id",
							"workspace_id",
							"title",
							"description",
							"requirements",
							"created_by",
							"creation_key",
						]
						.map(Alias::new),
					)
					.select_from(source)
					.unwrap()
					.to_string(PostgresQueryBuilder),
			)
			.bind(task.parse::<uuid::Uuid>().unwrap())
			.bind(child)
			.execute(&f.store.pool)
			.await
			.unwrap();
			let fields = [
				"id",
				"tenant",
				"policy_id",
				"policy_revision",
				"task_id",
				"workspace_id",
				"credential_id",
				"root_subject",
				"subject_chain",
				"agent_id",
				"agent_version",
				"definition",
				"status",
				"reason",
				"depth",
				"token_limit",
				"expires_at",
			];
			let mut source = Query::select();
			for field in fields {
				match field {
					"id" => {
						source.expr(Expr::cust("gen_random_uuid()"));
					}
					"task_id" => {
						source.expr(Expr::cust("$2"));
					}
					"root_subject" => {
						source.expr(Expr::val("hidden"));
					}
					"agent_id" => {
						source.expr(Expr::val(format!("hidden-{child}")));
					}
					_ => {
						source.column(Alias::new(field));
					}
				}
			}
			source
				.from(Alias::new("generation_requests"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")));
			sqlx::query(
				&Query::insert()
					.into_table(Alias::new("generation_requests"))
					.columns(fields.map(Alias::new))
					.select_from(source)
					.unwrap()
					.to_string(PostgresQueryBuilder),
			)
			.bind(job_id)
			.bind(child)
			.execute(&f.store.pool)
			.await
			.unwrap();
		}
	}
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/tasks/{other_task}/claim"),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}})
		)
		.await
		.0,
		200
	);
	let run = f.store.runs().await.unwrap().remove(0);
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("authorization_workspaces"))
			.value(
				sea_orm::sea_query::Alias::new("owner_subject"),
				sea_orm::sea_query::Expr::cust("'bob'"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("workspace_id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(run.workspace_id)
	.execute(&f.store.pool)
	.await
	.unwrap();
	policy["policies"].as_array_mut().unwrap().extend([
        json!({"id":"hidden-generations","effect":"deny","subjects":{"any":true},"actions":["generation.read"],"resources":{"kinds":["generation"]},"condition":{"op":"eq","left":{"source":"resource","path":"/root_subject"},"right":{"source":"literal","value":"hidden"}}}),
        json!({"id":"other-owner","effect":"deny","subjects":{"any":true},"actions":["run.read"],"resources":{"kinds":["run"]},"condition":{"op":"eq","left":{"source":"resource","path":"/owner"},"right":{"source":"literal","value":"bob"}}}),
    ]);
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
	let (status, visible) = request(
		&app,
		&token,
		"GET",
		"/api/generation/acme/requests",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{visible}");
	assert_eq!(visible.as_array().unwrap().len(), 1);
	assert_eq!(visible[0]["id"], job["id"]);
	let after: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("MAX(sequence)"))
			.from(sea_orm::sea_query::Alias::new("events"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	let mut tx = f.store.pool.begin().await.unwrap();
	f.store
		.event(
			&mut tx,
			Some(job["workspace_id"].as_str().unwrap().parse().unwrap()),
			"generation.changed",
			json!({"id":job_id}),
		)
		.await
		.unwrap();
	f.store
		.event(
			&mut tx,
			Some(run.workspace_id),
			"run.updated",
			json!({"run_id":run.id}),
		)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	let (status, events) = request(
		&app,
		&token,
		"GET",
		&format!("/api/events?after={after}"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{events}");
	assert_eq!(
		events.as_array().unwrap().len(),
		1,
		"the preceding generation context must not overwrite the next run's owner"
	);
	assert_eq!(events[0]["kind"], "generation.changed");
	cleanup(f, &url, &schema).await;
}
