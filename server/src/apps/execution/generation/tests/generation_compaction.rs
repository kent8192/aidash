#[path = "../../tests/support/upstream.rs"]
mod upstream_fixtures;
use futures_util::{FutureExt, future::BoxFuture};
use reinhardt::ServerRouter as Router;

use upstream_fixtures::{async_upstream, handler};
#[path = "../../tests/support/worker_process.rs"]
mod worker_process;
use worker_process::WorkerProcess;
#[path = "../../tests/support/legacy.rs"]
mod common;
use aidash_server::{federation::Federation, harness::Harness};

use common::*;

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
		let query_bind_2 = common::context(json!({"history":history}));
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
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
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
	#[from(provider_1)]
	fixture: Provider1Fixture,
) {
	let (f, url, schema) = fixture.application.runtime.parts();
	let app = fixture.application.application;
	let server = fixture.server;
	let endpoint = server.url.clone();
	let calls = fixture.calls;
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
	drop(server);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn compaction_total_budget_is_atomic_and_unused_calls_release_once(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
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
	#[from(provider_2)]
	fixture: Provider2Fixture,
) {
	let (f, url, schema) = fixture.application.runtime.parts();
	let app = fixture.application.application;
	let server = fixture.server;
	let endpoint = server.url.clone();
	let calls = fixture.calls;
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
	drop(server);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
#[case(false)]
#[case(true)]
async fn compaction_denial_and_catalog_revocation_prevent_disclosure(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
	#[case] revoke_catalog: bool,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
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

type PreparedCompaction =
	futures_util::future::Shared<BoxFuture<'static, (Provider3Fixture, String, Value)>>;
#[rstest::fixture]
fn queued_compaction(
	#[from(provider_3)] provider: BoxFuture<'static, Provider3Fixture>,
) -> PreparedCompaction {
	async move {
		let fixture = provider.await;
		let f = &fixture.application.runtime.federation;
		let app = &fixture.application.application;
		let (token, _) = policy(f, app, &fixture.server.url).await;
		let (_, assigned) = assign(app, &token).await;
		assert_eq!(assigned["generation"]["status"], "QUEUED");
		assert!(f.store.runs().await.unwrap().is_empty());
		(fixture, token, assigned["generation"].clone())
	}
	.boxed()
	.shared()
}
#[rstest::fixture]
fn compaction_worker_configuration(
	queued_compaction: PreparedCompaction,
) -> worker_process::ConfigurationFuture {
	async move {
		let (fixture, _, _) = queued_compaction.await;
		let (f, url, schema) = fixture.application.runtime.parts();
		(f, url, schema, None)
	}
	.boxed()
	.shared()
}

#[rstest::rstest]
#[tokio::test]
async fn process_restart_preserves_provisioning_and_uncertain_compaction_charge(
	queued_compaction: PreparedCompaction,
	#[from(compaction_worker_configuration)]
	#[with(queued_compaction.clone())]
	_configuration: worker_process::ConfigurationFuture,
	#[future(awt)]
	#[from(worker_process::worker_process)]
	#[with(_configuration.clone())]
	initial_worker: WorkerProcess,
	#[future(awt)]
	#[from(worker_process::worker_command)]
	#[with(_configuration.clone())]
	first_restart: worker_process::WorkerCommand,
	#[future(awt)]
	#[from(worker_process::worker_command)]
	#[with(_configuration.clone())]
	second_restart: worker_process::WorkerCommand,
) {
	let (fixture, token, job) = queued_compaction.await;
	let (f, url, schema) = fixture.application.runtime.parts();
	let app = fixture.application.application;
	let server = fixture.server;
	let first_model = fixture.first_model;
	let first_compaction = fixture.first_compaction;
	let model_calls = fixture.model_calls;
	let compaction_calls = fixture.compaction_calls;
	let mut worker = initial_worker;
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
	// Restart after SIGKILL is the recovery behavior under test.
	let worker = WorkerProcess::spawn(first_restart);
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
	// A second restart exercises settlement after the uncertain provider call.
	let worker = WorkerProcess::spawn(second_restart);
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
	drop(server);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
#[case(true)]
#[case(false)]
async fn nested_generation_intersects_compaction_approval_and_charges_both_ancestors(
	#[future(awt)]
	#[from(provider_4)]
	fixture: Provider4Fixture,
	#[case] approved: bool,
) {
	let (f, url, schema) = fixture.application.runtime.parts();
	let app = fixture.application.application;
	let server = fixture.server;
	let endpoint = server.url.clone();
	let calls = fixture.calls;
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
	drop(server);
	cleanup(f, &url, &schema).await;
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::SimpleExpr;

use reinhardt::query::Expr;

#[rstest::fixture]
fn provider_1_calls() -> Arc<std::sync::atomic::AtomicUsize> {
	Arc::new(AtomicUsize::new(0))
}

#[rstest::fixture]
fn provider_1_router(
	#[from(provider_1_calls)] calls: Arc<std::sync::atomic::AtomicUsize>,
	runtime: common::RuntimeFuture,
) -> upstream_fixtures::RouterFuture {
	async move {
	let f = runtime.await.federation;
	let pool = f.store.pool.driver().clone();

	let seen = calls.clone();
	let app = Router::new().handler("/systemone", handler(http::Method::POST, move |request: reinhardt::Request| {let body = request.json::<Value>().unwrap();
        let seen = seen.clone(); let pool = pool.clone(); async move {
            seen.fetch_add(1, Ordering::SeqCst);
            let attempts: i64 = sqlx::query_scalar(&reinhardt::query::Query::select().expr(reinhardt::query::Expr::cust("COUNT(*)")).from(reinhardt::query::Alias::new("generation_compaction_usage")).to_string(reinhardt::query::PostgresQueryBuilder)).fetch_one(&pool).await.unwrap();
            assert_eq!(attempts, 1, "reservation must be committed before HTTP");
            assert_eq!(body["model"], "fixture-jev");
            let answers: serde_json::Map<_,_> = body["questions"].as_object().unwrap().keys().map(|key| (key.clone(), json!({"noul":0.0}))).collect();
            reinhardt::Response::ok().with_json(&json!({"answers":answers})).unwrap()
        }
    })).handler("/v1/chat/completions", handler(http::Method::POST, |_request: reinhardt::Request| async { reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Compacted result"}}],"usage":{"prompt_tokens":100,"completion_tokens":10}})).unwrap() }));



Arc::new(app) }.boxed().shared()
}

struct Provider1Fixture {
	application: common::ApplicationFixture,
	server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[rstest::fixture]
fn provider_1(
	#[from(provider_1_calls)] calls: Arc<std::sync::atomic::AtomicUsize>,
	#[from(common::runtime)] _runtime: common::RuntimeFuture,
	#[from(provider_1_router)]
	#[with(calls.clone(), _runtime.clone())]
	_router: upstream_fixtures::RouterFuture,
	#[from(async_upstream)]
	#[with(_router.clone())]
	server: upstream_fixtures::UpstreamFuture,
	#[from(common::native_application)]
	#[with(Default::default(), aidash_server::sse::Service::new(Default::default()), Arc::new(|router| router), _runtime.clone())]
	application: common::ApplicationFuture,
) -> BoxFuture<'static, Provider1Fixture> {
	async move {
		Provider1Fixture {
			application: application.await,
			server: server.await,
			calls,
		}
	}
	.boxed()
}

#[rstest::fixture]
fn provider_2_calls() -> Arc<std::sync::atomic::AtomicUsize> {
	Arc::new(AtomicUsize::new(0))
}

#[rstest::fixture]
fn provider_2_router(
	#[from(provider_2_calls)] calls: Arc<std::sync::atomic::AtomicUsize>,
) -> upstream_fixtures::RouterFuture {
	async move {
		let seen = calls.clone();
		let server = Router::new().handler(
			"/systemone",
			handler(http::Method::POST, move |_request: reinhardt::Request| {
				let seen = seen.clone();
				async move {
					seen.fetch_add(1, Ordering::SeqCst);
					reinhardt::Response::new(http::StatusCode::TOO_MANY_REQUESTS)
				}
			}),
		);

		Arc::new(server)
	}
	.boxed()
	.shared()
}

struct Provider2Fixture {
	application: common::ApplicationFixture,
	server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[rstest::fixture]
fn provider_2(
	#[from(provider_2_calls)] calls: Arc<std::sync::atomic::AtomicUsize>,
	#[from(common::runtime)] _runtime: common::RuntimeFuture,
	#[from(provider_2_router)]
	#[with(calls.clone())]
	_router: upstream_fixtures::RouterFuture,
	#[from(async_upstream)]
	#[with(_router.clone())]
	server: upstream_fixtures::UpstreamFuture,
	#[from(common::native_application)]
	#[with(Default::default(), aidash_server::sse::Service::new(Default::default()), Arc::new(|router| router), _runtime.clone())]
	application: common::ApplicationFuture,
) -> BoxFuture<'static, Provider2Fixture> {
	async move {
		Provider2Fixture {
			application: application.await,
			server: server.await,
			calls,
		}
	}
	.boxed()
}

#[rstest::fixture]
fn provider_3_first_model() -> Arc<tokio::sync::Notify> {
	Arc::new(tokio::sync::Notify::new())
}

#[rstest::fixture]
fn provider_3_first_compaction() -> Arc<tokio::sync::Notify> {
	Arc::new(tokio::sync::Notify::new())
}

#[rstest::fixture]
fn provider_3_model_calls() -> Arc<std::sync::atomic::AtomicUsize> {
	Arc::new(AtomicUsize::new(0))
}

#[rstest::fixture]
fn provider_3_compaction_calls() -> Arc<std::sync::atomic::AtomicUsize> {
	Arc::new(AtomicUsize::new(0))
}

#[rstest::fixture]
fn provider_3_router(
	#[from(provider_3_first_model)] first_model: Arc<tokio::sync::Notify>,
	#[from(provider_3_first_compaction)] first_compaction: Arc<tokio::sync::Notify>,
	#[from(provider_3_model_calls)] model_calls: Arc<std::sync::atomic::AtomicUsize>,
	#[from(provider_3_compaction_calls)] compaction_calls: Arc<std::sync::atomic::AtomicUsize>,
	runtime: common::RuntimeFuture,
) -> upstream_fixtures::RouterFuture {
	async move {
	let f = runtime.await.federation;




	let server = Router::new()
        .handler("/v1/chat/completions", handler(http::Method::POST, {
            let started = first_model.clone(); let calls = model_calls.clone();
            move |_request: reinhardt::Request| { let started=started.clone(); let calls=calls.clone(); async move {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    started.notify_one(); std::future::pending::<()>().await;
                }
                reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Recovered result"}}],"usage":{"prompt_tokens":100,"completion_tokens":10}})).unwrap()
            }}
        }))
        .handler("/systemone", handler(http::Method::POST, {
            let started=first_compaction.clone(); let calls=compaction_calls.clone(); let pool=f.store.pool.driver().clone();
            move |request: reinhardt::Request| {let body = request.json::<Value>().unwrap(); let started=started.clone(); let calls=calls.clone(); let pool=pool.clone(); async move {
                let number=calls.fetch_add(1, Ordering::SeqCst)+1;
                let committed:i64=sqlx::query_scalar(&reinhardt::query::Query::select().expr(reinhardt::query::Expr::cust("COUNT(*)")).from(reinhardt::query::Alias::new("generation_compaction_usage")).to_string(reinhardt::query::PostgresQueryBuilder)).fetch_one(&pool).await.unwrap();
                assert_eq!(committed,number as i64);
                if number == 1 { started.notify_one(); std::future::pending::<()>().await; }
                let answers:serde_json::Map<_,_>=body["questions"].as_object().unwrap().keys().map(|k|(k.clone(),json!({"noul":0.0}))).collect();
                reinhardt::Response::ok().with_json(&json!({"answers":answers})).unwrap()
            }}
        }));



Arc::new(server) }.boxed().shared()
}

#[derive(Clone)]
struct Provider3Fixture {
	application: common::ApplicationFixture,
	server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	first_model: Arc<tokio::sync::Notify>,
	first_compaction: Arc<tokio::sync::Notify>,
	model_calls: Arc<std::sync::atomic::AtomicUsize>,
	compaction_calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[rstest::fixture]
fn provider_3(
	#[from(provider_3_first_model)] first_model: Arc<tokio::sync::Notify>,
	#[from(provider_3_first_compaction)] first_compaction: Arc<tokio::sync::Notify>,
	#[from(provider_3_model_calls)] model_calls: Arc<std::sync::atomic::AtomicUsize>,
	#[from(provider_3_compaction_calls)] compaction_calls: Arc<std::sync::atomic::AtomicUsize>,
	#[from(common::runtime)] _runtime: common::RuntimeFuture,
	#[from(provider_3_router)]
	#[with(first_model.clone(), first_compaction.clone(), model_calls.clone(), compaction_calls.clone(), _runtime.clone())]
	_router: upstream_fixtures::RouterFuture,
	#[from(async_upstream)]
	#[with(_router.clone())]
	server: upstream_fixtures::UpstreamFuture,
	#[from(common::native_application)]
	#[with(Default::default(), aidash_server::sse::Service::new(Default::default()), Arc::new(|router| router), _runtime.clone())]
	application: common::ApplicationFuture,
) -> BoxFuture<'static, Provider3Fixture> {
	async move {
		Provider3Fixture {
			application: application.await,
			server: server.await,
			first_model,
			first_compaction,
			model_calls,
			compaction_calls,
		}
	}
	.boxed()
}

#[rstest::fixture]
fn provider_4_calls() -> Arc<std::sync::atomic::AtomicUsize> {
	Arc::new(AtomicUsize::new(0))
}

#[rstest::fixture]
fn provider_4_router(
	#[from(provider_4_calls)] calls: Arc<std::sync::atomic::AtomicUsize>,
	runtime: common::RuntimeFuture,
) -> upstream_fixtures::RouterFuture {
	async move {

		let f = runtime.await.federation;

		let seen = calls.clone();
		let pool = f.store.pool.driver().clone();
		let server=Router::new().handler("/systemone",handler(http::Method::POST, move |request: reinhardt::Request| {let body = request.json::<Value>().unwrap();
            let seen=seen.clone(); let pool=pool.clone(); async move {
                seen.fetch_add(1,Ordering::SeqCst);
                let charges:i64=sqlx::query_scalar(&reinhardt::query::Query::select().expr(reinhardt::query::Expr::cust("COUNT(*)")).from(reinhardt::query::Alias::new("generation_compaction_usage")).to_string(reinhardt::query::PostgresQueryBuilder)).fetch_one(&pool).await.unwrap();
                assert_eq!(charges,2,"both ancestor reservations precede disclosure");
                let answers:serde_json::Map<_,_>=body["questions"].as_object().unwrap().keys().map(|k|(k.clone(),json!({"noul":0.0}))).collect();
                reinhardt::Response::ok().with_json(&json!({"answers":answers})).unwrap()
            }
        })).handler("/v1/chat/completions",handler(http::Method::POST, |_request: reinhardt::Request| async{reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Nested result"}}],"usage":{"prompt_tokens":100,"completion_tokens":10}})).unwrap()}));



Arc::new(server) }.boxed().shared()
}

struct Provider4Fixture {
	application: common::ApplicationFixture,
	server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[rstest::fixture]
fn provider_4(
	#[from(provider_4_calls)] calls: Arc<std::sync::atomic::AtomicUsize>,
	#[from(common::runtime)] _runtime: common::RuntimeFuture,
	#[from(provider_4_router)]
	#[with(calls.clone(), _runtime.clone())]
	_router: upstream_fixtures::RouterFuture,
	#[from(async_upstream)]
	#[with(_router.clone())]
	server: upstream_fixtures::UpstreamFuture,
	#[from(common::native_application)]
	#[with(Default::default(), aidash_server::sse::Service::new(Default::default()), Arc::new(|router| router), _runtime.clone())]
	application: common::ApplicationFuture,
) -> BoxFuture<'static, Provider4Fixture> {
	async move {
		Provider4Fixture {
			application: application.await,
			server: server.await,
			calls,
		}
	}
	.boxed()
}
