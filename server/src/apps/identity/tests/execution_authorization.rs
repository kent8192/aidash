#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
use aidash_application::tenancy::TEST_FIXTURE;
use common::upstream_fixtures;
use futures_util::{FutureExt, future::BoxFuture};
use http::Method;
use upstream_fixtures::{async_upstream, reply};
#[path = "../../execution/tests/support/legacy.rs"]
mod common;
use aidash_server::{domain::qualified_agent, harness::Harness};

use common::*;
use serde_json::{Value, json};
use std::sync::{
	Arc,
	atomic::{AtomicUsize, Ordering},
};
use uuid::Uuid;

#[rstest::rstest]
#[tokio::test]
async fn scoped_worker_recovers_from_a_missing_skill_path_and_reads_an_approved_file(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (mut policy, token, task_id) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let deployment_administrator = &f.config.api_token;
	let agent_subject = qualified_agent(&f.config.node_id, "skilled", "1.0.0");
	policy["subjects"][&agent_subject] = json!({"kind":"agent"});
	assert_eq!(
		request(
			&app,
			deployment_administrator,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy})
		)
		.await
		.0,
		200
	);
	for (kind, id, config) in [
		(
			"skill",
			"guide",
			json!({"instructions":"Read references/guide.md","files":[{"path":"references/guide.md","content":"Approved guide"}]}),
		),
		(
			"agent",
			"skilled",
			json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Read the guide","schema_version":1,"bindings":[{"kind":"skill","target":{"registry_node":f.config.node_id,"id":"guide","version":"1.0.0"},"narrow":{}}],"remove_default":[]}),
		),
	] {
		let entry = json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"fixture"},"capabilities":[],"languages":["en"],"schema":{"type":"object"},"config":config});
		let (status, body) = request(&app, &token, "POST", "/api/registry", entry).await;
		assert_eq!(status, 200, "registry: {body}");
		let (status, body) = request(
			&app,
			&token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":id,"version":"1.0.0"},"expected_revision":0,"enabled":true}),
		)
		.await;
		assert_eq!(status, 200, "catalog: {body}");
	}
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{task_id}/claim"),
		json!({"revision":0,"agent":{"id":"skilled","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "claim: {body}");
	let run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	let response = json!({"text":"","tool_calls":[
		{"id":"read-missing","name":"skill_read","arguments":{"skill":{"id":"guide","version":"1.0.0"},"path":"references/missing.md"}},
		{"id":"read-guide","name":"skill_read","arguments":{"skill":{"id":"guide","version":"1.0.0"},"path":"references/guide.md"}}
	],"input_tokens":0,"output_tokens":0});
	{
		let query_bind_1 = run.id;
		let query_bind_2 = common::tool_pending(
			json!({"response":response,"cursor":0,"request_window":120000,"request_tokens":0}),
		);
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("phase"),
					reinhardt::query::Expr::cust("'TOOL_CALL'"),
				)
				.value_expr(
					reinhardt::query::Alias::new("pending"),
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
		.execute(f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert!(
		Harness {
			federation: f.clone()
		}
		.worker_once()
		.await
		.unwrap()
	);
	let run = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	assert_eq!(
		json!(run.context)["history"][0]["event"]["result"]["error"],
		"Skill file not found: references/missing.md"
	);
	assert!(
		Harness {
			federation: f.clone()
		}
		.worker_once()
		.await
		.unwrap()
	);
	let run = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	assert_eq!(
		json!(run.context)["history"][1]["event"]["result"]["text"],
		"Approved guide"
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn scoped_worker_preserves_pending_tool_across_revocation_and_resumes_with_intersected_authority(
	#[future(awt)] #[from(scoped_worker_preserves_pending_tool_across_revocation_and_resumes_with_intersected_authority_provider)] fixture: ScopedWorkerPreservesPendingToolAcrossRevocationAndResumesWithIntersectedAuthorityProvider,
) {
	let (f, url, schema) = fixture.application.runtime.parts();
	let app = fixture.application.application;
	let server = fixture.server;
	let endpoint = server.url.clone();
	let effects = fixture.effects;
	let (mut policy, token, task_id) =
		bootstrap_requiring_tool_call_approval(&f, &app, &endpoint).await;
	let (status, _) = request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{task_id}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(
		status, 200,
		"a scoped claim must persist an execution identity"
	);
	let harness = Harness {
		federation: f.clone(),
	};
	assert!(harness.worker_once().await.unwrap());
	assert!(harness.worker_once().await.unwrap());
	let run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	assert_eq!(run.phase().as_str(), "TOOL_CALL");
	assert_eq!(effects.load(Ordering::SeqCst), 0);
	let agent = qualified_agent(&f.config.node_id, "research", "1.0.0");
	policy["subjects"][&agent]["enabled"] = json!(false);
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
	assert!(harness.worker_once().await.unwrap());
	let paused = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	assert_eq!(
		paused.control.as_str(),
		"PAUSED",
		"phase={} error={:?} pending={}",
		paused.phase().as_str(),
		paused.error,
		json!(paused.state)["data"]
	);
	assert_eq!(
		json!(paused.state)["data"],
		json!(run.state)["data"],
		"revocation must not discard the durable tool cursor"
	);
	assert_eq!(effects.load(Ordering::SeqCst), 0);
	assert!(!harness.worker_once().await.unwrap());
	policy["subjects"][&agent]["enabled"] = json!(true);
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"agent-tool-deny","effect":"deny","subjects":{"ids":[agent]},"actions":["tool.invoke"],"resources":{"kinds":["tool"]}}));
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
	assert!(harness.worker_once().await.unwrap());
	assert_eq!(
		f.store
			.run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.control
			.as_str(),
		"PAUSED"
	);
	assert_eq!(
		effects.load(Ordering::SeqCst),
		0,
		"root allow must not override agent deny"
	);
	policy["policies"].as_array_mut().unwrap().pop();
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":3,"bundle":policy})
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
	let restarted = Harness {
		federation: f.clone(),
	};
	for _ in 0..12 {
		restarted.worker_once().await.unwrap();
		let current = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
		if let Some((approval, call)) = awaited_tool_call(&current) {
			assert_eq!(call.name, "plugin_0");
			assert_eq!(call.id, "call-1");
			let (status, decided) = decide_tool_call(&app, &token, approval, "allow_once").await;
			assert_eq!(status, 200, "{decided}");
		}
		if current.phase().as_str() == "COMPLETED" {
			break;
		}
	}
	assert_eq!(
		f.store
			.run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.phase()
			.as_str(),
		"COMPLETED"
	);
	assert_eq!(
		f.store
			.task(&TEST_FIXTURE, task_id)
			.await
			.unwrap()
			.status
			.as_str(),
		"COMPLETED"
	);
	assert_eq!(effects.load(Ordering::SeqCst), 1);
	assert_eq!(
		f.store
			.snapshot(&TEST_FIXTURE, run.workspace_id)
			.await
			.unwrap()
			.artifacts
			.len(),
		1
	);
	drop(server);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn catalog_approval_and_run_read_denials_cover_search_collections_and_event_replay(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	use futures_util::StreamExt;
	use std::time::Duration;
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (mut policy, token, task_id) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let builtin_count = aidash_application::registry::system::operations().count();
	let (status, catalog) = request(&app, &token, "GET", "/api/registry", Value::Null).await;
	assert_eq!(status, 200, "{catalog}");
	assert_eq!(catalog.as_array().unwrap().len(), 3 + builtin_count);
	assert_eq!(
		request(&app, &token, "POST", "/api/discover", json!({}))
			.await
			.1["agents"]
			.as_array()
			.unwrap()
			.len(),
		1
	);
	let mut other = policy.clone();
	other["tenant"] = json!("other");
	common::tenant_root(&f, "other").await;
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/other",
			json!({"expected_revision":0,"bundle":other})
		)
		.await
		.0,
		200
	);
	let (_, other_credential) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/other/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	let other_token = other_credential["token"].as_str().unwrap();
	let (status, other_catalog) =
		request(&app, other_token, "GET", "/api/registry", Value::Null).await;
	assert_eq!(status, 200, "{other_catalog}");
	let other_catalog = other_catalog.as_array().unwrap();
	assert_eq!(other_catalog.len(), builtin_count);
	assert!(
		other_catalog
			.iter()
			.all(|entry| { entry["tags"].as_array().unwrap().contains(&json!("system")) })
	);
	assert_eq!(
		request(
			&app,
			other_token,
			"GET",
			"/api/registry/research/1.0.0",
			Value::Null
		)
		.await
		.0,
		403
	);
	let catalog =
		json!({"entry":{"id":"research","version":"1.0.0"},"enabled":false,"expected_revision":1});
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			"/api/authorization/acme/catalog",
			catalog.clone()
		)
		.await
		.0,
		200
	);
	let claim = json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}});
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/tasks/{task_id}/claim"),
			claim.clone()
		)
		.await
		.0,
		403
	);
	assert_eq!(
		f.store
			.task(&TEST_FIXTURE, task_id)
			.await
			.unwrap()
			.status
			.as_str(),
		"OPEN"
	);
	assert!(f.store.runs(&TEST_FIXTURE).await.unwrap().is_empty());
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			"/api/discover",
			json!({"query":"research"})
		)
		.await
		.1["agents"],
		json!([])
	);
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"research","version":"1.0.0"},"enabled":true,"expected_revision":2})
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
			"/api/authorization/acme/catalog",
			catalog
		)
		.await
		.0,
		409
	);
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/tasks/{task_id}/claim"),
			claim
		)
		.await
		.0,
		200
	);
	let run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	{
		let query_bind_1 = run.id;
		let query_bind_2 = json!({"private":"unreadable-run-journal"});
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
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
		.execute(f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	for path in [
		format!("/api/tasks/{task_id}/claim"),
		format!("/api/tasks/{task_id}/delegate"),
	] {
		assert_eq!(
			request(
				&app,
				other_token,
				"POST",
				&path,
				json!({"revision":0,"node_id":f.config.node_id,"agent":{"id":"research","version":"1.0.0"}})
			)
			.await
			.0,
			403,
			"foreign task status must not be exposed through an admission conflict"
		);
	}
	f.store
		.human_request(
			&TEST_FIXTURE,
			&run,
			"QUESTION",
			"unreadable-run-journal",
			"private-question",
		)
		.await
		.unwrap();
	let after: i64 = sqlx::query_scalar(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::Expr::cust("MAX(sequence)"))
			.from(reinhardt::query::Alias::new("events"))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_one(f.store.pool.fixture_driver())
	.await
	.unwrap();
	{
		let query_bind_1 = &f.config.node_id;
		let query_bind_2 = run.workspace_id;
		let query_bind_3 = json!({"run_id":run.id,"private":"unreadable-run-journal"});
		sqlx::query(
			&reinhardt::query::Query::insert()
				.into_table(reinhardt::query::Alias::new("events"))
				.columns([
					reinhardt::query::Alias::new("id"),
					reinhardt::query::Alias::new("node_id"),
					reinhardt::query::Alias::new("workspace_id"),
					reinhardt::query::Alias::new("kind"),
					reinhardt::query::Alias::new("data"),
					reinhardt::query::Alias::new("tenant"),
				])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(reinhardt::query::Expr::cust("GEN_RANDOM_UUID()"))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(reinhardt::query::Expr::cust("'model.completed'"))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.from_subquery(
							reinhardt::query::Query::select()
								.expr(reinhardt::query::Expr::cust("generate_series(1, 501)"))
								.to_owned(),
							reinhardt::query::Alias::new("i"),
						)
						.expr(Expr::value("acme"))
						.to_owned(),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	f.store
		.message(
			&TEST_FIXTURE,
			run.workspace_id,
			"alice",
			"permitted message after rejected events",
			Some("execution-authorization-unrelated-message"),
		)
		.await
		.unwrap();
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-run-read","effect":"deny","subjects":{"any":true},"actions":["run.read"],"resources":{"kinds":["run"],"ids":[run.id.to_string()]}}));
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
			&token,
			"GET",
			&format!("/api/runs/{}", run.id),
			Value::Null
		)
		.await
		.0,
		403
	);
	for path in [
		"/api/state".to_owned(),
		format!("/api/workspaces/{}", run.workspace_id),
		format!("/api/events?after={after}"),
	] {
		let (status, value) = request(&app, &token, "GET", &path, Value::Null).await;
		assert_eq!(status, 200, "{path}: {value}");
		assert!(
			!value.to_string().contains("unreadable-run-journal"),
			"leak through {path}"
		);
		if path == "/api/state" {
			assert_eq!(value["runs"], json!([]));
			assert_eq!(value["human_requests"], json!([]));
		}
		if path.starts_with("/api/events?") {
			assert_eq!(
				value.as_array().unwrap().len(),
				1,
				"hidden events must not trap pagination"
			);
		}
	}
	// Unbounded SSE uses the declared streaming HTTP client.
	let response = app
		.streaming_http
		.clone()
		.request(
			Method::GET,
			app.url(format!(
				"/api/events/stream?workspace_id={}&after={after}",
				run.workspace_id
			)),
		)
		.header("authorization", format!("Bearer {token}"))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 200);
	let mut stream = response.bytes_stream();
	let frame = tokio::time::timeout(Duration::from_secs(5), stream.next())
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	let text = String::from_utf8_lossy(&frame);
	assert!(text.contains("permitted message after rejected events"));
	assert!(!text.contains("unreadable-run-journal"));
	drop(stream);
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/runs/{}/control", run.id),
			json!({"action":"pause"})
		)
		.await
		.0,
		403,
		"control responses must not expose a denied run journal"
	);
	assert_eq!(
		f.store
			.inspect_run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.control
			.as_str(),
		"ACTIVE"
	);
	f.store
		.control(
			&TEST_FIXTURE,
			run.id,
			aidash_server::domain::RunControlAction::Pause,
		)
		.await
		.unwrap();
	let (_, second) = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{}/tasks", run.workspace_id),
		json!({"title":"observe","description":"inspect the permitted workspace"}),
	)
	.await;
	let second_id: Uuid = second["id"].as_str().unwrap().parse().unwrap();
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/tasks/{second_id}/claim"),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}})
		)
		.await
		.0,
		200
	);
	let observer = f
		.store
		.runs(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_iter()
		.find(|r| r.task_id == second_id)
		.unwrap();
	{ let query_bind_1 = observer.id; let query_bind_2 = common::tool_pending(json!({"response":{"text":"","tool_calls":[{"id":"observe","name":"workspace_observe","arguments":{}}],"input_tokens":0,"output_tokens":0},"cursor":0,"request_window":120000,"request_tokens":0})); sqlx::query(&reinhardt::query::Query::update().table(reinhardt::query::Alias::new("runs")).value_expr(reinhardt::query::Alias::new("phase"), reinhardt::query::Expr::cust("'TOOL_CALL'")).value_expr(reinhardt::query::Alias::new("pending"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_2.to_owned()).into()])).and_where(SimpleExpr::CustomWithExpr("(id = ?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()])).to_string(reinhardt::query::PostgresQueryBuilder))
        .execute(f.store.pool.fixture_driver()).await }.unwrap();
	Harness {
		federation: f.clone(),
	}
	.worker_once()
	.await
	.unwrap();
	let result: Value = {
		let query_bind_1 = observer.id;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("result")),
				))
				.from(reinhardt::query::Alias::new("invocations"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(run_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert!(
		!result.to_string().contains("unreadable-run-journal"),
		"worker observation must apply the same run visibility as API snapshots"
	);
	assert!(
		result
			.to_string()
			.contains("permitted message after rejected events")
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn child_execution_retains_parent_authority_and_supports_credential_rotation_and_cancellation(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (mut policy, token, task_id) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let child = qualified_agent(&f.config.node_id, "child", "1.0.0");
	policy["subjects"][&child] = json!({"kind":"agent"});
	let parent = qualified_agent(&f.config.node_id, "research", "1.0.0");
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"parent-tool-deny","effect":"deny","subjects":{"ids":[parent]},"actions":["tool.invoke"],"resources":{"kinds":["tool"],"ids":[format!("{}/tools/http@1.0.0",f.config.node_id)]}}));
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
	let mut entry = serde_json::to_value(
		f.registry
			.get(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				"research",
				"1.0.0",
			)
			.await
			.unwrap(),
	)
	.unwrap();
	entry["id"] = json!("child");
	entry
		.as_object_mut()
		.unwrap()
		.remove("binding_normalization");
	assert_eq!(
		request(&app, &token, "POST", "/api/registry", entry)
			.await
			.0,
		200
	);
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"child","version":"1.0.0"},"expected_revision":0,"enabled":true})
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
			&format!("/api/tasks/{task_id}/claim"),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}})
		)
		.await
		.0,
		200
	);
	let harness = Harness {
		federation: f.clone(),
	};
	harness.worker_once().await.unwrap();
	let parent_run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	let (_, task) = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{}/tasks", parent_run.workspace_id),
		json!({"title":"Child","description":"Delegated work","parent_id":task_id}),
	)
	.await;
	let child_task = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
	let response = json!({"text":"","tool_calls":[{"id":"delegate","name":"task_delegate","arguments":{"task_id":child_task,"node_id":f.config.node_id,"agent":{"id":"child","version":"1.0.0"}}}],"input_tokens":0,"output_tokens":0});
	{
		let query_bind_1 = parent_run.id;
		let query_bind_2 = common::tool_pending(json!({"response":response,"cursor":0}));
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("phase"),
					reinhardt::query::Expr::cust("'TOOL_CALL'"),
				)
				.value_expr(
					reinhardt::query::Alias::new("pending"),
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
		.execute(f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	harness.worker_once().await.unwrap();
	let child_run = f
		.store
		.runs(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_iter()
		.find(|r| r.task_id == child_task)
		.expect("parent tool must admit a child run");
	let chain: Vec<String> = {
		let query_bind_1 = child_run.id;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("subject_chain")),
				))
				.from(reinhardt::query::Alias::new("authorization_execution"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(run_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert_eq!(chain, vec!["alice".to_owned(), parent, child]);
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/runs/{}/control", parent_run.id),
			json!({"action":"pause"})
		)
		.await
		.0,
		200
	);
	harness.worker_once().await.unwrap();
	let response = json!({"text":"","tool_calls":[{"id":"effect","name":"plugin_0","arguments":{}}],"input_tokens":0,"output_tokens":0});
	{
		let query_bind_1 = child_run.id;
		let query_bind_2 = common::tool_pending(json!({"response":response,"cursor":0}));
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("phase"),
					reinhardt::query::Expr::cust("'TOOL_CALL'"),
				)
				.value_expr(
					reinhardt::query::Alias::new("pending"),
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
		.execute(f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	Harness {
		federation: f.clone(),
	}
	.worker_once()
	.await
	.unwrap();
	assert_eq!(
		f.store
			.run(&TEST_FIXTURE, child_run.id)
			.await
			.unwrap()
			.control
			.as_str(),
		"PAUSED",
		"parent deny must intersect the child grant after restart"
	);
	let invocations: i64 = {
		let query_bind_1 = child_run.id;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust("COUNT(*)"))
				.from(reinhardt::query::Alias::new("invocations"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(run_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert_eq!(
		invocations, 0,
		"authorization must precede durable invocation admission"
	);
	let (_, credentials) = request(
		&app,
		&f.config.api_token,
		"GET",
		"/api/authorization/acme/credentials",
		Value::Null,
	)
	.await;
	let old_id = credentials[0]["id"].as_str().unwrap();
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			&format!("/api/authorization/acme/credentials/{old_id}/revoke"),
			json!({})
		)
		.await
		.0,
		200
	);
	let (_, fresh) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	assert_eq!(
		request(
			&app,
			fresh["token"].as_str().unwrap(),
			"POST",
			&format!("/api/runs/{}/control", child_run.id),
			json!({"action":"resume"})
		)
		.await
		.0,
		200
	);
	let credential: Uuid = {
		let query_bind_1 = child_run.id;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("credential_id")),
				))
				.from(reinhardt::query::Alias::new("authorization_execution"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(run_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert_eq!(
		credential.to_string(),
		fresh["credential"]["id"].as_str().unwrap()
	);
	// Deployment Administrator cancellation is cleanup and must remain possible after every
	// source credential has been revoked; it must never invoke the provider.
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			&format!("/api/authorization/acme/credentials/{credential}/revoke"),
			json!({})
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
			&format!("/api/runs/{}/control", child_run.id),
			json!({"action":"cancel"})
		)
		.await
		.0,
		200
	);
	harness.worker_once().await.unwrap();
	assert_eq!(
		f.store
			.run(&TEST_FIXTURE, child_run.id)
			.await
			.unwrap()
			.phase()
			.as_str(),
		"CANCELLED"
	);
	assert_eq!(
		f.store
			.task(&TEST_FIXTURE, child_task)
			.await
			.unwrap()
			.status
			.as_str(),
		"CANCELLED"
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn worker_effect_boundary_serializes_revocation_and_persists_audit_before_the_effect(
	#[future(awt)]
	#[from(
		worker_effect_boundary_serializes_revocation_and_persists_audit_before_the_effect_provider
	)]
	fixture: WorkerEffectBoundarySerializesRevocationAndPersistsAuditBeforeTheEffectProvider,
) {
	use std::time::Duration;
	let (f, url, schema) = fixture.application.runtime.parts();
	let app = fixture.application.application;
	let server = fixture.server;
	let endpoint = server.url.clone();
	let entered = fixture.entered;
	let release = fixture.release;
	let worker_federation = fixture.worker_federation;
	let (mut policy, token, task_id) =
		bootstrap_requiring_tool_call_approval(&f, &app, &endpoint).await;
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/tasks/{task_id}/claim"),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}})
		)
		.await
		.0,
		200
	);
	let harness = Harness {
		federation: worker_federation.clone(),
	};
	harness.worker_once().await.unwrap();
	let run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	let response = json!({"text":"","tool_calls":[{"id":"effect","name":"plugin_0","arguments":{}}],"input_tokens":0,"output_tokens":0});
	{
		let query_bind_1 = run.id;
		let query_bind_2 = common::tool_pending(json!({"response":response,"cursor":0}));
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("phase"),
					reinhardt::query::Expr::cust("'TOOL_CALL'"),
				)
				.value_expr(
					reinhardt::query::Alias::new("pending"),
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
		.execute(f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	// Generic HTTP has no admitted replay guarantee and requires an exact
	// human decision before the serialization boundary is exercised.
	harness.worker_once().await.unwrap();
	let waiting = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	let (approval, call) = awaited_tool_call(&waiting)
		.unwrap_or_else(|| panic!("expected a Tool call approval, got {:?}", waiting.state));
	assert_eq!(call.name, "plugin_0");
	assert_eq!(call.id, "effect");
	let (status, decided) = decide_tool_call(&app, &token, approval, "allow_once").await;
	assert_eq!(status, 200, "{decided}");
	harness.worker_once().await.unwrap();
	assert!(matches!(
		f.store.run(&TEST_FIXTURE, run.id).await.unwrap().state,
		aidash_domain::RunState::ToolCall(_)
	));
	let audit_query =
		reinhardt::query::Query::select()
			.expr(reinhardt::query::Expr::cust("COUNT(*)"))
			.from(reinhardt::query::Alias::new("authorization_decisions"))
			.and_where(reinhardt::query::Expr::cust(
				"action = 'tool.invoke' AND resource_id = 'aidash://execution-test/tools/http@1.0.0' AND decision ->> 'allowed' = 'true'",
			))
			.to_string(reinhardt::query::PostgresQueryBuilder);
	let before_effect: i64 = sqlx::query_scalar(&audit_query)
		.fetch_one(f.store.pool.fixture_driver())
		.await
		.unwrap();
	let worker = tokio::spawn(async move { harness.worker_once().await });
	tokio::time::timeout(Duration::from_secs(5), entered.notified())
		.await
		.unwrap();
	let audited: i64 = sqlx::query_scalar(&audit_query)
		.fetch_one(f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(
		audited - before_effect,
		2,
		"root and agent authorization must be durable before the HTTP effect starts"
	);
	let (_, credentials) = request(
		&app,
		&f.config.api_token,
		"GET",
		"/api/authorization/acme/credentials",
		Value::Null,
	)
	.await;
	let revoke_path = format!(
		"/api/authorization/acme/credentials/{}/revoke",
		credentials[0]["id"].as_str().unwrap()
	);
	policy["subjects"][qualified_agent(&f.config.node_id, "research", "1.0.0")]["enabled"] =
		json!(false);
	let mut replacements = tokio::task::JoinSet::new();
	// Native ORM mutations acquire SELECT FOR UPDATE locks before writing.
	// Fill every API connection with a waiting revocation. The worker must
	// still persist its result and release the lease using its separate pool.
	for _ in 0..11 {
		let app = app.clone();
		let deployment_administrator = f.config.api_token.clone();
		let policy = policy.clone();
		replacements.spawn(async move {
			request(
				&app,
				&deployment_administrator,
				"POST",
				"/api/authorization/acme",
				json!({"expected_revision":1,"bundle":policy}),
			)
			.await
		});
	}
	let revoke = tokio::spawn({
		let app = app.clone();
		let deployment_administrator = f.config.api_token.clone();
		async move {
			request(
				&app,
				&deployment_administrator,
				"POST",
				&revoke_path,
				json!({}),
			)
			.await
		}
	});
	tokio::time::timeout(Duration::from_secs(5),async {
        loop {
            let waiting:i64={ let query_bind_1 = &schema; sqlx::query_scalar(&reinhardt::query::Query::select().expr(reinhardt::query::Expr::cust("COUNT(*)")).from(reinhardt::query::Alias::new("pg_stat_activity")).and_where(SimpleExpr::CustomWithExpr("(application_name = ? AND wait_event_type = 'Lock' AND (query LIKE '%authorization_bundles%' OR query LIKE '%authorization_credentials%'))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()])).to_string(reinhardt::query::PostgresQueryBuilder)).fetch_one(worker_federation.store.pool.fixture_driver()).await }.unwrap();
            if waiting==12 {break;}
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("revocation was not serialized with the in-flight effect boundary");
	release.notify_one();
	assert!(
		tokio::time::timeout(Duration::from_secs(5), worker)
			.await
			.unwrap()
			.unwrap()
			.unwrap()
	);
	let mut replaced = 0;
	while let Some(result) = replacements.join_next().await {
		match result.unwrap().0 {
			200 => replaced += 1,
			409 => (),
			status => panic!("unexpected replacement status: {status}"),
		}
	}
	assert_eq!(replaced, 1);
	assert_eq!(revoke.await.unwrap().0, 200);
	let invocation: String = {
		let query_bind_1 = run.id;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("status")),
				))
				.from(reinhardt::query::Alias::new("invocations"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(run_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert_eq!(invocation, "COMPLETED");
	Harness {
		federation: worker_federation.clone(),
	}
	.worker_once()
	.await
	.unwrap();
	let paused = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	assert_eq!(
		paused.control.as_str(),
		"PAUSED",
		"phase={} error={:?} pending={}",
		paused.phase().as_str(),
		paused.error,
		json!(paused.state)["data"]
	);
	assert_eq!(json!(paused.state)["data"]["cursor"], 1);
	drop(server);
	worker_federation.store.pool.close().await;
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn scoped_delegation_requires_permission_before_atomic_admission(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (mut policy, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-delegation","effect":"deny","subjects":{"ids":["alice"]},"actions":["task.delegate"],"resources":{"kinds":["task"]}}));
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
			&token,
			"POST",
			&format!("/api/tasks/{task}/delegate"),
			json!({"node_id":f.config.node_id,"agent":{"id":"research","version":"1.0.0"}})
		)
		.await
		.0,
		403
	);
	assert_eq!(
		f.store
			.task(&TEST_FIXTURE, task)
			.await
			.unwrap()
			.status
			.as_str(),
		"OPEN"
	);
	assert!(f.store.runs(&TEST_FIXTURE).await.unwrap().is_empty());
	let count: i64 = sqlx::query_scalar(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::Expr::cust("COUNT(*)"))
			.from(reinhardt::query::Alias::new("delegations"))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_one(f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(count, 0);
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
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn scoped_collections_fill_after_denied_runs_and_stream_cursor_skips_denied_tail(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	use aidash_server::authorization::{Authorization, identity::Actor, workspace::Workspaces};
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (mut policy, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
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
	let visible = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	let denied: Vec<Uuid> = (0..501).map(|_| Uuid::new_v4()).collect();
	{
		use reinhardt::query::{Alias, IntoValue, PostgresQueryBuilder, Query};
		let mut insert = Query::insert();
		insert.into_table(Alias::new("runs")).columns(
			[
				"id",
				"task_id",
				"workspace_id",
				"home_node",
				"agent_id",
				"agent_version",
				"updated_at",
				"tenant",
			]
			.map(Alias::new),
		);
		let newer = chrono::Utc::now() + chrono::Duration::seconds(1);
		for id in &denied {
			insert
				.values_expr(vec![
					IntoValue::into_value(*id).into(),
					IntoValue::into_value(Uuid::new_v4()).into(),
					IntoValue::into_value(visible.workspace_id).into(),
					IntoValue::into_value(f.config.node_id.as_str()).into(),
					IntoValue::into_value("research").into(),
					IntoValue::into_value("1.0.0").into(),
					IntoValue::into_value(newer).into(),
					Expr::value("acme").into(),
				])
				.expect("one value per column");
		}
		sqlx::query(&insert.to_string(PostgresQueryBuilder))
			.execute(f.store.pool.fixture_driver())
			.await
			.unwrap();
	}
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"hidden-runs","effect":"deny","subjects":{"any":true},"actions":["run.read"],"resources":{"kinds":["run"],"ids":denied}}));
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
	let (status, state) = request(&app, &token, "GET", "/api/state", Value::Null).await;
	assert_eq!(status, 200, "{state}");
	assert_eq!(state["runs"].as_array().unwrap().len(), 1);
	assert_eq!(state["runs"][0]["id"], visible.id.to_string());
	let before: i64 = sqlx::query_scalar(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::Expr::cust("COALESCE(MAX(sequence), 0)"))
			.from(reinhardt::query::Alias::new("events"))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_one(f.store.pool.fixture_driver())
	.await
	.unwrap();
	let last = f
		.store
		.emit(
			&TEST_FIXTURE,
			Some(visible.workspace_id),
			"run.tool_recorded",
			json!({"run_id":denied[0]}),
		)
		.await
		.unwrap();
	let Actor::Subject(identity) = (Authorization {
		pool: f.store.pool.clone(),
	})
	.authenticate(&token)
	.await
	.unwrap() else {
		panic!("subject expected")
	};
	let scope = Workspaces {
		store: f.store.clone(),
		identity,
	};
	let (events, cursor) = scope.poll_events(before, None, 100).await.unwrap();
	assert!(events.is_empty());
	assert_eq!(cursor, last.sequence);
	assert_eq!(
		scope.poll_events(cursor, None, 100).await.unwrap().1,
		cursor
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn malformed_scoped_delegation_arguments_remain_model_correctable(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
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
	let harness = Harness {
		federation: f.clone(),
	};
	harness.worker_once().await.unwrap();
	let run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	let response = aidash_server::provider::ModelResponse {
		tool_calls: vec![aidash_server::provider::ToolCall {
			id: "bad-id".into(),
			name: "task_delegate".into(),
			arguments: json!({"task_id":"not-a-uuid","node_id":f.config.node_id,"agent":{"id":"research","version":"1.0.0"}}),
		}],
		..Default::default()
	};
	{
		let query_bind_1 = run.id;
		let query_bind_2 = common::tool_pending(json!({"response":response,"cursor":0}));
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs"))
				.value_expr(
					reinhardt::query::Alias::new("phase"),
					reinhardt::query::Expr::cust("'TOOL_CALL'"),
				)
				.value_expr(
					reinhardt::query::Alias::new("pending"),
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
		.execute(f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	harness.worker_once().await.unwrap();
	let run = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	assert_eq!(run.phase().as_str(), "TOOL_CALL");
	assert_eq!(json!(run.state)["data"]["cursor"], 1);
	assert!(json!(run.context)["history"][0]["event"]["result"]["error"].is_string());
	assert_eq!(
		f.store
			.task(&TEST_FIXTURE, task)
			.await
			.unwrap()
			.status
			.as_str(),
		"RUNNING"
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn decision_cursor_follows_transaction_commit_order(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	use aidash_server::authorization::{Authorization, policy::Evaluation};
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let authorization = Authorization {
		pool: f.store.pool.clone(),
	};
	let evaluation:Evaluation=serde_json::from_value(json!({"subject":"alice","action":"workspace.read","resource":{"tenant":"acme","kind":"workspace","id":"cursor-test","attributes":{}},"environment":{}})).unwrap();
	let mut tx = f.store.pool.begin(&TEST_FIXTURE).await.unwrap();
	Authorization::evaluate_in_transaction(&mut tx, "acme", &evaluation)
		.await
		.unwrap();
	let second = tokio::spawn(async move { authorization.evaluate("acme", &evaluation).await });
	tokio::time::sleep(std::time::Duration::from_millis(50)).await;
	assert!(
		!second.is_finished(),
		"later decisions must wait until the first allocation commits"
	);
	tx.commit().await.unwrap();
	second.await.unwrap().unwrap();
	let rows: Vec<(i64, String)> = sqlx::query_as(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::SimpleExpr::from(
				reinhardt::query::Expr::col(reinhardt::query::Alias::new("sequence")),
			))
			.expr(reinhardt::query::SimpleExpr::from(
				reinhardt::query::Expr::col(reinhardt::query::Alias::new("resource_id")),
			))
			.from(reinhardt::query::Alias::new("authorization_decisions"))
			.and_where(reinhardt::query::Expr::cust("resource_id = 'cursor-test'"))
			.order_by_expr(
				reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
					reinhardt::query::Alias::new("sequence"),
				)),
				reinhardt::query::Order::Asc,
			)
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_all(f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(rows.len(), 2);
	assert!(rows[0].0 < rows[1].0);
	cleanup(f, &url, &schema).await;
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::SimpleExpr;

use reinhardt::query::Expr;

#[rstest::rstest]
#[tokio::test]
async fn catalog_history_failure_rolls_back_the_approval_and_preserves_retry(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	use aidash_server::{authorization::Authorization, registry::EntityRef};
	use reinhardt::query::{
		Alias, Expr, ExprTrait, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
	};
	// Arrange: reserve the future history key in an isolated database to make
	// history fail after the approval CAS has successfully updated its row.
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	bootstrap(&f, &app, "http://localhost:9").await;
	let authorization = Authorization {
		pool: f.store.pool.clone(),
	};
	let reference = EntityRef {
		id: "model".into(),
		version: "1.0.0".into(),
	};
	let collision = Query::insert()
		.into_table(Alias::new("authorization_catalog_history"))
		.columns(
			[
				"tenant",
				"entry_id",
				"entry_version",
				"revision",
				"enabled",
				"actor",
			]
			.map(Alias::new),
		)
		.values(vec![
			"acme".into(),
			"model".into(),
			"1.0.0".into(),
			2_i64.into(),
			false.into(),
			"collision-fixture".into(),
		])
		.unwrap()
		.to_string(PostgresQueryBuilder);
	sqlx::query(&collision)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	// Act
	let error = authorization
		.set_catalog("acme", &reference, 1, false, "deployment_administrator")
		.await
		.unwrap_err();
	// Assert: the original database constraint, not a synthetic application
	// conflict, causes rollback of the already successful approval update.
	let aidash_server::Error::Framework(error) = error else {
		panic!("unexpected failure: {error:?}")
	};
	assert_eq!(error.database_error().unwrap().code(), Some("23505"));
	let binding: (i64, bool) = sqlx::query_as(
		"SELECT revision, enabled FROM authorization_catalog WHERE tenant = 'acme' AND entry_id = 'model'",
	)
	.fetch_one(f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(binding, (1, true));
	let history_query = Query::select()
		.columns(["revision", "enabled", "actor"].map(Alias::new))
		.from(Alias::new("authorization_catalog_history"))
		.and_where(Expr::col("tenant").eq(Expr::value("acme")))
		.and_where(Expr::col("entry_id").eq(Expr::value("model")))
		.and_where(Expr::col("entry_version").eq(Expr::value("1.0.0")))
		.order_by(Alias::new("revision"), Order::Asc)
		.to_string(PostgresQueryBuilder);
	let history: Vec<(i64, bool, String)> = sqlx::query_as(&history_query)
		.fetch_all(f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(
		history,
		vec![
			(1, true, "deployment_administrator".into()),
			(2, false, "collision-fixture".into())
		]
	);
	// Remove only this test's collision. The same expected revision can then
	// complete, proving the failed transaction did not consume that revision.
	let remove = Query::delete()
		.from_table(Alias::new("authorization_catalog_history"))
		.and_where(Expr::col("tenant").eq(Expr::value("acme")))
		.and_where(Expr::col("entry_id").eq(Expr::value("model")))
		.and_where(Expr::col("entry_version").eq(Expr::value("1.0.0")))
		.and_where(Expr::col("revision").eq(Expr::value(2_i64)))
		.to_string(PostgresQueryBuilder);
	assert_eq!(
		sqlx::query(&remove)
			.execute(f.store.pool.fixture_driver())
			.await
			.unwrap()
			.rows_affected(),
		1
	);
	let binding = authorization
		.set_catalog(
			"acme",
			&reference,
			1,
			false,
			"deployment-administrator-retry",
		)
		.await
		.unwrap();
	assert_eq!(binding.revision, 2);
	assert!(!binding.enabled);
	let history: Vec<(i64, bool, String)> = sqlx::query_as(&history_query)
		.fetch_all(f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(
		history,
		vec![
			(1, true, "deployment_administrator".into()),
			(2, false, "deployment-administrator-retry".into())
		]
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::fixture]
fn scoped_worker_preserves_pending_tool_across_revocation_and_resumes_with_intersected_authority_provider_effects()
-> Arc<AtomicUsize> {
	Arc::new(AtomicUsize::new(0))
}

#[rstest::fixture]
fn scoped_worker_preserves_pending_tool_across_revocation_and_resumes_with_intersected_authority_provider_router(
	#[from(scoped_worker_preserves_pending_tool_across_revocation_and_resumes_with_intersected_authority_provider_effects)]
	effects: Arc<AtomicUsize>,
) -> upstream_fixtures::RouterFuture {
	async move {


	let counter = effects.clone();
	let server=reinhardt::test::stub::StubRouter::new()
.route("/effect", http::Method::POST, reply(move |_request: reinhardt::Request| { let counter=counter.clone(); async move {
        counter.fetch_add(1,Ordering::SeqCst); reinhardt::Response::ok().with_json(&json!({"saved":true})).unwrap()
    }}))
.route("/v1/chat/completions", http::Method::POST, reply(|request: reinhardt::Request| {let body = request.json::<Value>().unwrap();async move {
        let context:Value=serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
        let message=if context["history"].as_array().unwrap().iter().any(|e| e["kind"]=="tool") {
            json!({"role":"assistant","content":"Completed authorized work"})
        } else { json!({"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"plugin_0","arguments":"{}"}}]}) };
        reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"},"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
    }})).into_server_router();



Arc::new(server)}.boxed().shared()
}

struct ScopedWorkerPreservesPendingToolAcrossRevocationAndResumesWithIntersectedAuthorityProvider {
	application: common::ApplicationFixture,
	server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	effects: Arc<AtomicUsize>,
}

#[rstest::fixture]
fn scoped_worker_preserves_pending_tool_across_revocation_and_resumes_with_intersected_authority_provider(
	#[from(scoped_worker_preserves_pending_tool_across_revocation_and_resumes_with_intersected_authority_provider_effects)]
	effects: Arc<AtomicUsize>,
	#[from(common::runtime)] _runtime: common::RuntimeFuture,
	#[from(scoped_worker_preserves_pending_tool_across_revocation_and_resumes_with_intersected_authority_provider_router)]
	#[with(effects.clone())]
	_router: upstream_fixtures::RouterFuture,
	#[from(async_upstream)]
	#[with(_router.clone())]
	_server: upstream_fixtures::UpstreamFuture,
	#[from(common::native_application)]
	#[with(Default::default(),aidash_server::sse::Service::new(Default::default()),Arc::new(|r|r),_runtime.clone())]
	_application: common::ApplicationFuture,
) -> BoxFuture<
	'static,
	ScopedWorkerPreservesPendingToolAcrossRevocationAndResumesWithIntersectedAuthorityProvider,
> {
	async move {
		ScopedWorkerPreservesPendingToolAcrossRevocationAndResumesWithIntersectedAuthorityProvider {
			application: _application.await,
			server: _server.await,
			effects,
		}
	}
	.boxed()
}

#[rstest::fixture]
fn worker_effect_boundary_serializes_revocation_and_persists_audit_before_the_effect_provider_entered()
-> Arc<tokio::sync::Notify> {
	Arc::new(tokio::sync::Notify::new())
}

#[rstest::fixture]
fn worker_effect_boundary_serializes_revocation_and_persists_audit_before_the_effect_provider_release()
-> Arc<tokio::sync::Notify> {
	Arc::new(tokio::sync::Notify::new())
}

#[rstest::fixture]
fn worker_effect_boundary_serializes_revocation_and_persists_audit_before_the_effect_provider_router(
	#[from(worker_effect_boundary_serializes_revocation_and_persists_audit_before_the_effect_provider_entered)]
	entered: Arc<tokio::sync::Notify>,
	#[from(worker_effect_boundary_serializes_revocation_and_persists_audit_before_the_effect_provider_release)]
	release: Arc<tokio::sync::Notify>,
) -> upstream_fixtures::RouterFuture {
	async move {
		let handler_entered = entered.clone();
		let handler_release = release.clone();

		let fixture = reinhardt::test::stub::StubRouter::new()
			.route(
				"/effect",
				http::Method::POST,
				reply(move |_request: reinhardt::Request| {
					let entered = handler_entered.clone();
					let release = handler_release.clone();
					async move {
						entered.notify_one();
						release.notified().await;
						reinhardt::Response::ok()
							.with_json(&json!({"effect":"committed"}))
							.unwrap()
					}
				}),
			)
			.into_server_router();

		Arc::new(fixture)
	}
	.boxed()
	.shared()
}

struct WorkerEffectBoundarySerializesRevocationAndPersistsAuditBeforeTheEffectProvider {
	application: common::ApplicationFixture,
	worker_federation: aidash_server::federation::Federation,
	server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	entered: Arc<tokio::sync::Notify>,
	release: Arc<tokio::sync::Notify>,
}

#[rstest::fixture]
fn worker_effect_boundary_serializes_revocation_and_persists_audit_before_the_effect_provider(
	#[from(worker_effect_boundary_serializes_revocation_and_persists_audit_before_the_effect_provider_entered)]
	entered: Arc<tokio::sync::Notify>,
	#[from(worker_effect_boundary_serializes_revocation_and_persists_audit_before_the_effect_provider_release)]
	release: Arc<tokio::sync::Notify>,
	#[from(common::runtime)] _runtime: common::RuntimeFuture,
	#[from(effect_worker_runtime)]
	#[with(_runtime.clone())]
	_worker: BoxFuture<'static, aidash_server::federation::Federation>,
	#[from(worker_effect_boundary_serializes_revocation_and_persists_audit_before_the_effect_provider_router)]
	#[with(entered.clone(), release.clone())]
	_router: upstream_fixtures::RouterFuture,
	#[from(async_upstream)]
	#[with(_router.clone())]
	_server: upstream_fixtures::UpstreamFuture,
	#[from(common::native_application)]
	#[with(Default::default(),aidash_server::sse::Service::new(Default::default()),Arc::new(|r|r),_runtime.clone())]
	_application: common::ApplicationFuture,
) -> BoxFuture<
	'static,
	WorkerEffectBoundarySerializesRevocationAndPersistsAuditBeforeTheEffectProvider,
> {
	async move {
		WorkerEffectBoundarySerializesRevocationAndPersistsAuditBeforeTheEffectProvider {
			application: _application.await,
			worker_federation: _worker.await,
			server: _server.await,
			entered,
			release,
		}
	}
	.boxed()
}

#[rstest::fixture]
fn effect_worker_runtime(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) -> BoxFuture<'static, aidash_server::federation::Federation> {
	async move { runtime.await.federation.for_workers().await.unwrap() }.boxed()
}

/// A scripted model that calls the Tenant's HTTP tool until it has called it
/// `script` times, then answers; both its upstream and the tool count their hits.
/// When the Agent also offers `plugin_1`, its first call is that read-only tool.
struct ToolCallApprovals {
	application: common::ApplicationFixture,
	server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	effects: Arc<AtomicUsize>,
	/// Hits of the read-only tool's `/read` route.
	reads: Arc<AtomicUsize>,
	inferences: Arc<AtomicUsize>,
	script: Arc<AtomicUsize>,
}

/// How often each route of the scripted upstream was hit.
#[derive(Clone, Default)]
struct Hits {
	effects: Arc<AtomicUsize>,
	inferences: Arc<AtomicUsize>,
	reads: Arc<AtomicUsize>,
}

#[rstest::fixture]
fn tool_call_hits() -> Hits {
	Hits::default()
}

#[rstest::fixture]
fn tool_call_script() -> Arc<AtomicUsize> {
	Arc::new(AtomicUsize::new(1))
}

#[rstest::fixture]
fn tool_call_router(
	#[from(tool_call_hits)] hits: Hits,
	#[from(tool_call_script)] script: Arc<AtomicUsize>,
) -> upstream_fixtures::RouterFuture {
	async move {
		let Hits {
			effects: counter,
			inferences,
			reads,
		} = hits;
		let server = reinhardt::test::stub::StubRouter::new()
			.route(
				"/read",
				http::Method::POST,
				reply(move |_request: reinhardt::Request| {
					let counter = reads.clone();
					async move {
						counter.fetch_add(1, Ordering::SeqCst);
						reinhardt::Response::ok()
							.with_json(&json!({"found":true}))
							.unwrap()
					}
				}),
			)
			.route(
				"/effect",
				http::Method::POST,
				reply(move |_request: reinhardt::Request| {
					let counter = counter.clone();
					async move {
						counter.fetch_add(1, Ordering::SeqCst);
						reinhardt::Response::ok()
							.with_json(&json!({"saved":true}))
							.unwrap()
					}
				}),
			)
			.route(
				"/v1/chat/completions",
				http::Method::POST,
				reply(move |request: reinhardt::Request| {
					let body = request.json::<Value>().unwrap();
					let inference = inferences.fetch_add(1, Ordering::SeqCst) + 1;
					let script = script.clone();
					async move {
						let context: Value =
							serde_json::from_str(body["messages"][1]["content"].as_str().unwrap())
								.unwrap();
						let called = context["history"]
							.as_array()
							.unwrap()
							.iter()
							.filter(|event| event["kind"] == "tool")
							.count();
						let reads_first = called == 0
							&& body["tools"].as_array().is_some_and(|tools| {
								tools
									.iter()
									.any(|tool| tool["function"]["name"] == "plugin_1")
							});
						let name = if reads_first { "plugin_1" } else { "plugin_0" };
						let message = if called < script.load(Ordering::SeqCst) {
							json!({"role":"assistant","content":null,"tool_calls":[{"id":format!("call-{inference}"),"type":"function","function":{"name":name,"arguments":json!({"call":called + 1}).to_string()}}]})
						} else {
							json!({"role":"assistant","content":"Completed authorized work"})
						};
						reinhardt::Response::ok()
							.with_json(&json!({"choices":[{"index":0,"finish_reason":if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"},"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
							.unwrap()
					}
				}),
			)
			.into_server_router();
		Arc::new(server)
	}
	.boxed()
	.shared()
}

#[rstest::fixture]
fn tool_call_approvals(
	#[from(tool_call_hits)] hits: Hits,
	#[from(tool_call_script)] script: Arc<AtomicUsize>,
	#[from(common::runtime)] _runtime: common::RuntimeFuture,
	#[from(tool_call_router)]
	#[with(hits.clone(), script.clone())]
	_router: upstream_fixtures::RouterFuture,
	#[from(async_upstream)]
	#[with(_router.clone())]
	_server: upstream_fixtures::UpstreamFuture,
	#[from(common::native_application)]
	#[with(Default::default(),aidash_server::sse::Service::new(Default::default()),Arc::new(|r|r),_runtime.clone())]
	_application: common::ApplicationFuture,
) -> BoxFuture<'static, ToolCallApprovals> {
	async move {
		ToolCallApprovals {
			application: _application.await,
			server: _server.await,
			effects: hits.effects,
			reads: hits.reads,
			inferences: hits.inferences,
			script,
		}
	}
	.boxed()
}

/// Step the worker until `done` holds for the Run, which must happen soon.
async fn step_until(
	harness: &Harness,
	f: &aidash_server::federation::Federation,
	run: Uuid,
	mut done: impl FnMut(&aidash_server::domain::Run) -> bool,
) -> aidash_server::domain::Run {
	for _ in 0..40 {
		harness.worker_once().await.unwrap();
		let current = f.store.run(&TEST_FIXTURE, run).await.unwrap();
		if done(&current) {
			return current;
		}
	}
	let current = f.store.run(&TEST_FIXTURE, run).await.unwrap();
	panic!(
		"the Run did not reach the expected state: phase={} error={:?} state={:?}",
		current.phase().as_str(),
		current.error,
		current.state
	);
}

/// Every approval card the reader sees, whatever its state.
async fn approval_cards(app: &common::TestApplication, token: &str, query: &str) -> Vec<Value> {
	let (status, page) = request(
		app,
		token,
		"GET",
		&format!("/api/capabilities/approvals{query}"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{page}");
	page["items"].as_array().unwrap().clone()
}

/// A member's claimed Run under a policy without direct permission to call tools.
async fn claimed_run(
	t: &ToolCallApprovals,
	f: &aidash_server::federation::Federation,
	policy: impl FnOnce(&mut Value),
) -> (String, aidash_server::domain::Run) {
	let app = &t.application.application;
	let (mut bundle, token, task_id) =
		bootstrap_requiring_tool_call_approval(f, app, &t.server.url).await;
	policy(&mut bundle);
	let (status, claimed) = request(
		app,
		&token,
		"POST",
		&format!("/api/tasks/{task_id}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{claimed}");
	let run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	(token, run)
}

#[rstest::rstest]
#[tokio::test]
async fn an_approved_tool_call_runs_exactly_once_without_another_model_response(
	#[future(awt)] tool_call_approvals: ToolCallApprovals,
) {
	let t = tool_call_approvals;
	let (f, url, schema) = t.application.runtime.parts();
	let (token, run) = claimed_run(&t, &f, |_| {}).await;
	let harness = Harness {
		federation: f.clone(),
	};
	let waiting = step_until(&harness, &f, run.id, |run| awaited_tool_call(run).is_some()).await;
	let (approval, call) = awaited_tool_call(&waiting).unwrap();
	assert_eq!(
		(call.name.as_str(), call.id.as_str()),
		("plugin_0", "call-1")
	);
	assert_eq!(
		t.effects.load(Ordering::SeqCst),
		0,
		"nothing runs before the decision"
	);
	assert_eq!(t.inferences.load(Ordering::SeqCst), 1);
	let pending = approval_cards(
		&t.application.application,
		&token,
		"?state=pending&role=approver",
	)
	.await;
	assert_eq!(pending.len(), 1, "{pending:?}");
	let card = &pending[0];
	assert_eq!(card["id"], json!(approval));
	assert_eq!(card["kind"], "tool_call");
	assert_eq!(card["tool"], "http");
	assert_eq!(card["version"], "1.0.0");
	assert_eq!(card["operation"], "invoke");
	assert_eq!(card["call"]["name"], "plugin_0");
	assert_eq!(card["call"]["arguments"], json!({"call":1}));
	assert_eq!(card["approver"], "alice");
	assert_eq!(card["viewer_is_approver"], true);
	let (status, decided) =
		decide_tool_call(&t.application.application, &token, approval, "allow_once").await;
	assert_eq!(status, 200, "{decided}");
	assert_eq!(decided["state"], "approved");
	let finished = step_until(&harness, &f, run.id, |run| {
		run.phase().as_str() == "COMPLETED"
	})
	.await;
	assert_eq!(t.effects.load(Ordering::SeqCst), 1);
	assert_eq!(
		t.inferences.load(Ordering::SeqCst),
		2,
		"the approved call runs as asked: the model is never asked to repeat it"
	);
	let tools: Vec<_> = finished
		.context
		.events()
		.filter_map(|event| match event {
			aidash_server::context::ContextEvent::Tool { call, result } => Some((call, result)),
			_ => None,
		})
		.collect();
	assert_eq!(tools.len(), 1, "{tools:?}");
	assert_eq!(tools[0].0.id, "call-1");
	assert!(
		!tools[0].1.to_string().contains("approval_required"),
		"{:?}",
		tools[0].1
	);
	let all = approval_cards(&t.application.application, &token, "").await;
	assert_eq!(all.len(), 1, "{all:?}");
	assert_eq!(
		all[0]["state"], "consumed",
		"the one call consumed the approval"
	);
	drop(t.server);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn allow_for_this_run_covers_the_next_call_of_the_tool_after_new_user_input(
	#[future(awt)] tool_call_approvals: ToolCallApprovals,
) {
	let t = tool_call_approvals;
	t.script.store(2, Ordering::SeqCst);
	let (f, url, schema) = t.application.runtime.parts();
	let (token, run) = claimed_run(&t, &f, |_| {}).await;
	let app = &t.application.application;
	let harness = Harness {
		federation: f.clone(),
	};
	let waiting = step_until(&harness, &f, run.id, |run| awaited_tool_call(run).is_some()).await;
	let (approval, _) = awaited_tool_call(&waiting).unwrap();
	let (status, decided) = decide_tool_call(app, &token, approval, "allow_run").await;
	assert_eq!(status, 200, "{decided}");
	let grant = decided["grant_id"].clone();
	assert!(grant.is_string(), "{decided}");
	let granted = approval_cards(app, &token, "?state=active").await;
	assert_eq!(granted.len(), 1, "{granted:?}");
	assert_eq!(granted[0]["id"], grant);
	assert_eq!(granted[0]["tool"], "http");
	assert_eq!(granted[0]["operation"], "invoke");
	step_until(&harness, &f, run.id, |run| {
		run.phase().as_str() != "TOOL_CALL" && awaited_tool_call(run).is_none()
	})
	.await;
	assert_eq!(t.effects.load(Ordering::SeqCst), 1);
	let (status, message) = request(
		app,
		&token,
		"POST",
		&format!("/api/runs/{}/message", run.id),
		json!({"content":"Call it once more"}),
	)
	.await;
	assert_eq!(status, 200, "{message}");
	let mut asked = false;
	let finished = step_until(&harness, &f, run.id, |run| {
		asked |= awaited_tool_call(run).is_some();
		run.phase().as_str() == "COMPLETED"
	})
	.await;
	assert!(
		!asked,
		"the Run grant covers the next call: {:?}",
		finished.state
	);
	assert_eq!(t.effects.load(Ordering::SeqCst), 2);
	let all = approval_cards(app, &token, "").await;
	let calls: Vec<_> = all
		.iter()
		.filter(|card| card["kind"] == "tool_call")
		.collect();
	assert_eq!(calls.len(), 2, "{all:?}");
	assert!(calls.iter().all(|card| card["state"] == "consumed"));
	assert!(
		calls
			.iter()
			.filter(|card| card["grant_id"] == grant)
			.count() == 2,
		"both calls ran under the one grant: {calls:?}"
	);
	drop(t.server);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_tenant_policy_allowing_unapproved_tool_calls_skips_the_approval(
	#[future(awt)] tool_call_approvals: ToolCallApprovals,
) {
	let t = tool_call_approvals;
	let (f, url, schema) = t.application.runtime.parts();
	let app = &t.application.application;
	// The common policy names `tool.call_unapproved`, which a wildcard never grants.
	let (_, token, task_id) = bootstrap(&f, app, &t.server.url).await;
	let (status, claimed) = request(
		app,
		&token,
		"POST",
		&format!("/api/tasks/{task_id}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{claimed}");
	let run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	let harness = Harness {
		federation: f.clone(),
	};
	let finished = step_until(&harness, &f, run.id, |run| {
		assert!(
			awaited_tool_call(run).is_none(),
			"direct permission asks nobody"
		);
		run.phase().as_str() == "COMPLETED"
	})
	.await;
	assert_eq!(finished.phase().as_str(), "COMPLETED");
	assert_eq!(t.effects.load(Ordering::SeqCst), 1);
	assert!(
		approval_cards(app, &token, "?state=pending")
			.await
			.is_empty()
	);
	drop(t.server);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_read_only_operation_runs_without_approval_while_another_tool_of_the_run_waits(
	#[future(awt)] tool_call_approvals: ToolCallApprovals,
) {
	let t = tool_call_approvals;
	t.script.store(2, Ordering::SeqCst);
	let (f, url, schema) = t.application.runtime.parts();
	let app = &t.application.application;
	let (_, token, task_id) = bootstrap_with_read_only_tool(&f, app, &t.server.url).await;
	let (status, claimed) = request(
		app,
		&token,
		"POST",
		&format!("/api/tasks/{task_id}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{claimed}");
	let run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	let harness = Harness {
		federation: f.clone(),
	};
	let waiting = step_until(&harness, &f, run.id, |run| awaited_tool_call(run).is_some()).await;
	let (approval, call) = awaited_tool_call(&waiting).unwrap();
	assert_eq!(
		call.name, "plugin_0",
		"only the tool that is not read-only waits"
	);
	assert_eq!(
		t.reads.load(Ordering::SeqCst),
		1,
		"the read-only call ran before the approval was raised"
	);
	assert_eq!(t.effects.load(Ordering::SeqCst), 0);
	let pending = approval_cards(app, &token, "?state=pending&role=approver").await;
	assert_eq!(pending.len(), 1, "{pending:?}");
	assert_eq!(
		(pending[0]["id"].clone(), &pending[0]["tool"]),
		(json!(approval), &json!("http"))
	);
	let (status, decided) = decide_tool_call(app, &token, approval, "allow_once").await;
	assert_eq!(status, 200, "{decided}");
	step_until(&harness, &f, run.id, |run| {
		run.phase().as_str() == "COMPLETED"
	})
	.await;
	assert_eq!(t.reads.load(Ordering::SeqCst), 1);
	assert_eq!(t.effects.load(Ordering::SeqCst), 1);
	let all = approval_cards(app, &token, "").await;
	assert_eq!(
		all.iter()
			.filter(|card| card["kind"] == "tool_call")
			.map(|card| card["tool"].clone())
			.collect::<Vec<_>>(),
		[json!("http")],
		"the read-only tool never raised an approval: {all:?}"
	);
	drop(t.server);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_denied_tool_call_is_refused_and_the_run_continues(
	#[future(awt)] tool_call_approvals: ToolCallApprovals,
) {
	let t = tool_call_approvals;
	let (f, url, schema) = t.application.runtime.parts();
	let (token, run) = claimed_run(&t, &f, |_| {}).await;
	let harness = Harness {
		federation: f.clone(),
	};
	let waiting = step_until(&harness, &f, run.id, |run| awaited_tool_call(run).is_some()).await;
	let (approval, _) = awaited_tool_call(&waiting).unwrap();
	let (status, decided) =
		decide_tool_call(&t.application.application, &token, approval, "deny").await;
	assert_eq!(status, 200, "{decided}");
	assert_eq!(decided["state"], "denied");
	let finished = step_until(&harness, &f, run.id, |run| {
		run.phase().as_str() == "COMPLETED"
	})
	.await;
	assert_eq!(
		t.effects.load(Ordering::SeqCst),
		0,
		"a denied call never runs"
	);
	let refusals: Vec<_> = finished
		.context
		.events()
		.filter_map(|event| match event {
			aidash_server::context::ContextEvent::Tool { result, .. } => Some(result.to_string()),
			_ => None,
		})
		.collect();
	assert_eq!(refusals.len(), 1, "{refusals:?}");
	assert!(
		refusals[0].contains("tool call approval was denied"),
		"{refusals:?}"
	);
	drop(t.server);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_revoked_tool_call_approval_refuses_the_call(
	#[future(awt)] tool_call_approvals: ToolCallApprovals,
) {
	let t = tool_call_approvals;
	let (f, url, schema) = t.application.runtime.parts();
	let (token, run) = claimed_run(&t, &f, |_| {}).await;
	let app = &t.application.application;
	let harness = Harness {
		federation: f.clone(),
	};
	let waiting = step_until(&harness, &f, run.id, |run| awaited_tool_call(run).is_some()).await;
	let (approval, _) = awaited_tool_call(&waiting).unwrap();
	let card = approval_cards(app, &token, "?state=pending&role=approver")
		.await
		.remove(0);
	let (status, revoked) = request(
		app,
		&token,
		"POST",
		&format!("/api/capabilities/approvals/{approval}/revoke"),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":card["revision"]}),
	)
	.await;
	assert_eq!(status, 200, "{revoked}");
	assert_eq!(revoked["state"], "revoked");
	let finished = step_until(&harness, &f, run.id, |run| {
		run.phase().as_str() == "COMPLETED"
	})
	.await;
	assert_eq!(
		t.effects.load(Ordering::SeqCst),
		0,
		"a revoked approval allows nothing"
	);
	assert!(
		finished
			.context
			.events()
			.any(|event| matches!(event, aidash_server::context::ContextEvent::Tool { result, .. } if result.to_string().contains("tool call approval was revoked")))
	);
	drop(t.server);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn an_expired_tool_call_approval_refuses_the_call(
	#[future(awt)] tool_call_approvals: ToolCallApprovals,
) {
	let t = tool_call_approvals;
	let (f, url, schema) = t.application.runtime.parts();
	let (_, run) = claimed_run(&t, &f, |_| {}).await;
	let harness = Harness {
		federation: f.clone(),
	};
	let waiting = step_until(&harness, &f, run.id, |run| awaited_tool_call(run).is_some()).await;
	let (approval, _) = awaited_tool_call(&waiting).unwrap();
	// The Deployment's `approval_seconds` bound the wait: let it lapse.
	sqlx::query(
		&reinhardt::query::Query::update()
			.table(reinhardt::query::Alias::new("core_records"))
			.value_expr(
				reinhardt::query::Alias::new("expires_at"),
				reinhardt::query::Expr::cust("clock_timestamp() - INTERVAL '1 SECOND'"),
			)
			.and_where(SimpleExpr::CustomWithExpr(
				"(id = ?)".to_owned(),
				vec![Expr::value(approval.to_owned()).into()],
			))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	let finished = step_until(&harness, &f, run.id, |run| {
		run.phase().as_str() == "COMPLETED"
	})
	.await;
	assert_eq!(
		t.effects.load(Ordering::SeqCst),
		0,
		"an expired approval allows nothing"
	);
	assert!(finished.context.events().any(|event| matches!(
		event,
		aidash_server::context::ContextEvent::Tool { result, .. }
			if result.to_string().contains("tool call approval expired")
	)));
	drop(t.server);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_pending_tool_call_approval_closes_when_its_run_ends(
	#[future(awt)] tool_call_approvals: ToolCallApprovals,
) {
	let t = tool_call_approvals;
	let (f, url, schema) = t.application.runtime.parts();
	let (token, run) = claimed_run(&t, &f, |_| {}).await;
	let app = &t.application.application;
	let harness = Harness {
		federation: f.clone(),
	};
	let waiting = step_until(&harness, &f, run.id, |run| awaited_tool_call(run).is_some()).await;
	let (approval, _) = awaited_tool_call(&waiting).unwrap();
	assert_eq!(
		request(
			app,
			&token,
			"POST",
			&format!("/api/runs/{}/control", run.id),
			json!({"action":"cancel"})
		)
		.await
		.0,
		200
	);
	assert!(
		approval_cards(app, &token, "?state=pending&role=approver")
			.await
			.is_empty(),
		"an inbox never offers an approval whose Run has ended"
	);
	let all = approval_cards(app, &token, "").await;
	assert_eq!(all.len(), 1, "{all:?}");
	assert_eq!(all[0]["id"], json!(approval));
	assert_eq!(all[0]["state"], "closed");
	assert_eq!(all[0]["closed_reason"], "run_ended");
	assert_eq!(t.effects.load(Ordering::SeqCst), 0);
	drop(t.server);
	cleanup(f, &url, &schema).await;
}
