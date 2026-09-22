mod common;
use aidash::{api, domain::qualified_agent, harness::Harness};
use axum::{Json, Router, body::Body, http::Request, routing::post};
use common::*;
use serde_json::{Value, json};
use std::sync::{
	Arc,
	atomic::{AtomicUsize, Ordering},
};
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn scoped_worker_preserves_pending_tool_across_revocation_and_resumes_with_intersected_authority()
 {
	let (f, url, schema) = setup().await;
	let effects = Arc::new(AtomicUsize::new(0));
	let counter = effects.clone();
	let server=Router::new().route("/effect",post(move || { let counter=counter.clone(); async move {
        counter.fetch_add(1,Ordering::SeqCst); Json(json!({"saved":true}))
    }})).route("/v1/chat/completions",post(|Json(body):Json<Value>| async move {
        let context:Value=serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
        let message=if context["history"].as_array().unwrap().iter().any(|e| e["kind"]=="tool") {
            json!({"role":"assistant","content":"Completed authorized work"})
        } else { json!({"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"plugin_0","arguments":"{}"}}]}) };
        Json(json!({"choices":[{"index":0,"finish_reason":if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"},"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
    }));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener, server).await.unwrap();
	});
	let app = api::router(f.clone());
	let (mut policy, token, task_id) = bootstrap(&f, &app, &endpoint).await;
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
	let run = f.store.runs().await.unwrap().remove(0);
	assert_eq!(run.phase, "TOOL_CALL");
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
	let paused = f.store.run(run.id).await.unwrap();
	assert_eq!(
		paused.control, "PAUSED",
		"phase={} error={:?} pending={}",
		paused.phase, paused.error, paused.pending
	);
	assert_eq!(
		paused.pending, run.pending,
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
	assert_eq!(f.store.run(run.id).await.unwrap().control, "PAUSED");
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
	for _ in 0..8 {
		restarted.worker_once().await.unwrap();
		if f.store.run(run.id).await.unwrap().phase == "COMPLETED" {
			break;
		}
	}
	assert_eq!(f.store.run(run.id).await.unwrap().phase, "COMPLETED");
	assert_eq!(f.store.task(task_id).await.unwrap().status, "COMPLETED");
	assert_eq!(effects.load(Ordering::SeqCst), 1);
	assert_eq!(
		f.store
			.snapshot(run.workspace_id)
			.await
			.unwrap()
			.artifacts
			.len(),
		1
	);
	server.abort();
	let _ = server.await;
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn catalog_approval_and_run_read_denials_cover_search_collections_and_event_replay() {
	use futures_util::StreamExt;
	use std::time::Duration;
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (mut policy, token, task_id) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	assert_eq!(
		request(&app, &token, "GET", "/api/registry", Value::Null)
			.await
			.1
			.as_array()
			.unwrap()
			.len(),
		3
	);
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
	assert_eq!(
		request(&app, other_token, "GET", "/api/registry", Value::Null).await,
		(200, json!([]))
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
			&f.config.api_token,
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
	assert_eq!(f.store.task(task_id).await.unwrap().status, "OPEN");
	assert!(f.store.runs().await.unwrap().is_empty());
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
			&f.config.api_token,
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
			&f.config.api_token,
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
	let run = f.store.runs().await.unwrap().remove(0);
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("runs"))
			.value(
				sea_orm::sea_query::Alias::new("context"),
				sea_orm::sea_query::Expr::cust("$2"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind(json!({"private":"unreadable-run-journal"}))
	.execute(&f.store.pool)
	.await
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
			&run,
			"QUESTION",
			"unreadable-run-journal",
			"private-question",
		)
		.await
		.unwrap();
	let after: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("MAX(sequence)"))
			.from(sea_orm::sea_query::Alias::new("events"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	sqlx::query(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new("events"))
			.columns([
				sea_orm::sea_query::Alias::new("id"),
				sea_orm::sea_query::Alias::new("node_id"),
				sea_orm::sea_query::Alias::new("workspace_id"),
				sea_orm::sea_query::Alias::new("kind"),
				sea_orm::sea_query::Alias::new("data"),
			])
			.select_from(
				sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::Expr::cust("GEN_RANDOM_UUID()"))
					.expr(sea_orm::sea_query::Expr::cust("$1"))
					.expr(sea_orm::sea_query::Expr::cust("$2"))
					.expr(sea_orm::sea_query::Expr::cust("'model.completed'"))
					.expr(sea_orm::sea_query::Expr::cust("$3"))
					.from_function(
						sea_orm::sea_query::Func::cust(sea_orm::sea_query::Alias::new(
							"generate_series",
						))
						.args([
							sea_orm::sea_query::Expr::val(1).into(),
							sea_orm::sea_query::Expr::val(501).into(),
						]),
						sea_orm::sea_query::Alias::new("i"),
					)
					.to_owned(),
			)
			.expect("valid insert projection")
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(&f.config.node_id)
	.bind(run.workspace_id)
	.bind(json!({"run_id":run.id,"private":"unreadable-run-journal"}))
	.execute(&f.store.pool)
	.await
	.unwrap();
	f.store
		.message(
			run.workspace_id,
			"alice",
			"permitted message after rejected events",
			None,
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
	let response = app
		.clone()
		.oneshot(
			Request::get(format!(
				"/api/events/stream?workspace_id={}&after={after}",
				run.workspace_id
			))
			.header("authorization", format!("Bearer {token}"))
			.body(Body::empty())
			.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(response.status(), 200);
	let mut stream = response.into_body().into_data_stream();
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
	assert_eq!(f.store.run(run.id).await.unwrap().control, "ACTIVE");
	f.store.control(run.id, "pause").await.unwrap();
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
		.runs()
		.await
		.unwrap()
		.into_iter()
		.find(|r| r.task_id == second_id)
		.unwrap();
	sqlx::query(&sea_orm::sea_query::Query::update().table(sea_orm::sea_query::Alias::new("runs")).value(sea_orm::sea_query::Alias::new("phase"), sea_orm::sea_query::Expr::cust("'TOOL_CALL'")).value(sea_orm::sea_query::Alias::new("pending"), sea_orm::sea_query::Expr::cust("$2")).and_where(sea_orm::sea_query::Expr::cust("id = $1")).to_string(sea_orm::sea_query::PostgresQueryBuilder))
		.bind(observer.id).bind(json!({"response":{"text":"","tool_calls":[{"id":"observe","name":"workspace_observe","arguments":{}}],"input_tokens":0,"output_tokens":0},"cursor":0,"request_window":120000,"request_tokens":0}))
        .execute(&f.store.pool).await.unwrap();
	Harness {
		federation: f.clone(),
	}
	.worker_once()
	.await
	.unwrap();
	let result: Value = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("result")),
			))
			.from(sea_orm::sea_query::Alias::new("invocations"))
			.and_where(sea_orm::sea_query::Expr::cust("run_id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(observer.id)
	.fetch_one(&f.store.pool)
	.await
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

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn child_execution_retains_parent_authority_and_supports_credential_rotation_and_cancellation()
 {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (mut policy, token, task_id) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let child = qualified_agent(&f.config.node_id, "child", "1.0.0");
	policy["subjects"][&child] = json!({"kind":"agent"});
	let parent = qualified_agent(&f.config.node_id, "research", "1.0.0");
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"parent-tool-deny","effect":"deny","subjects":{"ids":[parent]},"actions":["tool.invoke"],"resources":{"kinds":["tool"],"ids":["http"]}}));
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
	let mut entry =
		serde_json::to_value(f.registry.get("research", "1.0.0").await.unwrap()).unwrap();
	entry["id"] = json!("child");
	assert_eq!(
		request(&app, &f.config.api_token, "POST", "/api/registry", entry)
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
	let parent_run = f.store.runs().await.unwrap().remove(0);
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
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("runs"))
			.value(
				sea_orm::sea_query::Alias::new("phase"),
				sea_orm::sea_query::Expr::cust("'TOOL_CALL'"),
			)
			.value(
				sea_orm::sea_query::Alias::new("pending"),
				sea_orm::sea_query::Expr::cust("$2"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(parent_run.id)
	.bind(json!({"response":response,"cursor":0}))
	.execute(&f.store.pool)
	.await
	.unwrap();
	harness.worker_once().await.unwrap();
	let child_run = f
		.store
		.runs()
		.await
		.unwrap()
		.into_iter()
		.find(|r| r.task_id == child_task)
		.expect("parent tool must admit a child run");
	let chain: Vec<String> = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("subject_chain")),
			))
			.from(sea_orm::sea_query::Alias::new("authorization_execution"))
			.and_where(sea_orm::sea_query::Expr::cust("run_id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(child_run.id)
	.fetch_one(&f.store.pool)
	.await
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
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("runs"))
			.value(
				sea_orm::sea_query::Alias::new("phase"),
				sea_orm::sea_query::Expr::cust("'TOOL_CALL'"),
			)
			.value(
				sea_orm::sea_query::Alias::new("pending"),
				sea_orm::sea_query::Expr::cust("$2"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(child_run.id)
	.bind(json!({"response":response,"cursor":0}))
	.execute(&f.store.pool)
	.await
	.unwrap();
	Harness {
		federation: f.clone(),
	}
	.worker_once()
	.await
	.unwrap();
	assert_eq!(
		f.store.run(child_run.id).await.unwrap().control,
		"PAUSED",
		"parent deny must intersect the child grant after restart"
	);
	let invocations: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
			.from(sea_orm::sea_query::Alias::new("invocations"))
			.and_where(sea_orm::sea_query::Expr::cust("run_id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(child_run.id)
	.fetch_one(&f.store.pool)
	.await
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
	let credential: Uuid = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("credential_id")),
			))
			.from(sea_orm::sea_query::Alias::new("authorization_execution"))
			.and_where(sea_orm::sea_query::Expr::cust("run_id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(child_run.id)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(
		credential.to_string(),
		fresh["credential"]["id"].as_str().unwrap()
	);
	// Operator cancellation is cleanup and must remain possible after every
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
	assert_eq!(f.store.run(child_run.id).await.unwrap().phase, "CANCELLED");
	assert_eq!(f.store.task(child_task).await.unwrap().status, "CANCELLED");
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn worker_effect_boundary_serializes_revocation_and_persists_audit_before_the_effect() {
	use std::time::Duration;
	let (f, url, schema) = setup().await;
	let worker_federation = f.for_workers().await.unwrap();
	let entered = Arc::new(tokio::sync::Notify::new());
	let release = Arc::new(tokio::sync::Notify::new());
	let handler_entered = entered.clone();
	let handler_release = release.clone();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let fixture = Router::new().route(
		"/effect",
		post(move || {
			let entered = handler_entered.clone();
			let release = handler_release.clone();
			async move {
				entered.notify_one();
				release.notified().await;
				Json(json!({"effect":"committed"}))
			}
		}),
	);
	let server = tokio::spawn(async move {
		axum::serve(listener, fixture).await.unwrap();
	});
	let app = api::router(f.clone());
	let (mut policy, token, task_id) = bootstrap(&f, &app, &endpoint).await;
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
	let run = f.store.runs().await.unwrap().remove(0);
	let response = json!({"text":"","tool_calls":[{"id":"effect","name":"plugin_0","arguments":{}}],"input_tokens":0,"output_tokens":0});
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("runs"))
			.value(
				sea_orm::sea_query::Alias::new("phase"),
				sea_orm::sea_query::Expr::cust("'TOOL_CALL'"),
			)
			.value(
				sea_orm::sea_query::Alias::new("pending"),
				sea_orm::sea_query::Expr::cust("$2"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind(json!({"response":response,"cursor":0}))
	.execute(&f.store.pool)
	.await
	.unwrap();
	let worker = tokio::spawn(async move { harness.worker_once().await });
	tokio::time::timeout(Duration::from_secs(5), entered.notified())
		.await
		.unwrap();
	let audited: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
			.from(sea_orm::sea_query::Alias::new("authorization_decisions"))
			.and_where(sea_orm::sea_query::Expr::cust(
				"action = 'tool.invoke' AND resource_id = 'http' AND decision ->> 'allowed' = 'true'",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(
		audited, 2,
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
	// Fill every API connection with a waiting revocation. The worker must
	// still persist its result and release the lease using its separate pool.
	for _ in 0..11 {
		let app = app.clone();
		let operator = f.config.api_token.clone();
		let policy = policy.clone();
		replacements.spawn(async move {
			request(
				&app,
				&operator,
				"POST",
				"/api/authorization/acme",
				json!({"expected_revision":1,"bundle":policy}),
			)
			.await
		});
	}
	let revoke = tokio::spawn({
		let app = app.clone();
		let operator = f.config.api_token.clone();
		async move { request(&app, &operator, "POST", &revoke_path, json!({})).await }
	});
	tokio::time::timeout(Duration::from_secs(5),async {
        loop {
            let waiting:i64=sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::Expr::cust("COUNT(*)")).from(sea_orm::sea_query::Alias::new("pg_stat_activity")).and_where(sea_orm::sea_query::Expr::cust("application_name = $1 AND wait_event_type = 'Lock' AND (REPLACE(query, CHR(34), '') LIKE 'UPDATE authorization_bundles%' OR REPLACE(query, CHR(34), '') LIKE 'UPDATE authorization_credentials%')")).to_string(sea_orm::sea_query::PostgresQueryBuilder))
                .bind(&schema).fetch_one(&worker_federation.store.pool).await.unwrap();
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
	let invocation: String = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("status")),
			))
			.from(sea_orm::sea_query::Alias::new("invocations"))
			.and_where(sea_orm::sea_query::Expr::cust("run_id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(run.id)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(invocation, "COMPLETED");
	Harness {
		federation: worker_federation.clone(),
	}
	.worker_once()
	.await
	.unwrap();
	let paused = f.store.run(run.id).await.unwrap();
	assert_eq!(
		paused.control, "PAUSED",
		"phase={} error={:?} pending={}",
		paused.phase, paused.error, paused.pending
	);
	assert_eq!(paused.pending["cursor"], 1);
	server.abort();
	let _ = server.await;
	worker_federation.store.pool.close().await;
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn scoped_delegation_requires_permission_before_atomic_admission() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
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
	assert_eq!(f.store.task(task).await.unwrap().status, "OPEN");
	assert!(f.store.runs().await.unwrap().is_empty());
	let count: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
			.from(sea_orm::sea_query::Alias::new("delegations"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
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

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn scoped_collections_fill_after_denied_runs_and_stream_cursor_skips_denied_tail() {
	use aidash::authorization::{Authorization, identity::Actor, workspace::Workspaces};
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
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
	let visible = f.store.runs().await.unwrap().remove(0);
	let denied: Vec<Uuid> = (0..501).map(|_| Uuid::new_v4()).collect();
	{
		use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
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
			]
			.map(Alias::new),
		);
		for id in &denied {
			insert.values_panic([
				Expr::cust(format!("'{id}'::uuid")),
				Expr::cust("gen_random_uuid()"),
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::val("research").into(),
				Expr::val("1.0.0").into(),
				Expr::cust("clock_timestamp()+interval '1 second'"),
			]);
		}
		sqlx::query(&insert.to_string(PostgresQueryBuilder))
			.bind(visible.workspace_id)
			.bind(&f.config.node_id)
			.execute(&f.store.pool)
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
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COALESCE(MAX(sequence), 0)"))
			.from(sea_orm::sea_query::Alias::new("events"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	let last = f
		.store
		.emit(
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

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn malformed_scoped_delegation_arguments_remain_model_correctable() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
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
	let run = f.store.runs().await.unwrap().remove(0);
	let response = aidash::provider::ModelResponse {
		tool_calls: vec![aidash::provider::ToolCall {
			id: "bad-id".into(),
			name: "task_delegate".into(),
			arguments: json!({"task_id":"not-a-uuid","node_id":f.config.node_id,"agent":{"id":"research","version":"1.0.0"}}),
		}],
		..Default::default()
	};
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("runs"))
			.value(
				sea_orm::sea_query::Alias::new("phase"),
				sea_orm::sea_query::Expr::cust("'TOOL_CALL'"),
			)
			.value(
				sea_orm::sea_query::Alias::new("pending"),
				sea_orm::sea_query::Expr::cust("$2"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind(json!({"response":response,"cursor":0}))
	.execute(&f.store.pool)
	.await
	.unwrap();
	harness.worker_once().await.unwrap();
	let run = f.store.run(run.id).await.unwrap();
	assert_eq!(run.phase, "TOOL_CALL");
	assert_eq!(run.pending["cursor"], 1);
	assert!(run.context["history"][0]["result"]["error"].is_string());
	assert_eq!(f.store.task(task).await.unwrap().status, "RUNNING");
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn decision_cursor_follows_transaction_commit_order() {
	use aidash::authorization::{Authorization, policy::Evaluation};
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let authorization = Authorization {
		pool: f.store.pool.clone(),
	};
	let evaluation:Evaluation=serde_json::from_value(json!({"subject":"alice","action":"workspace.read","resource":{"tenant":"acme","kind":"workspace","id":"cursor-test","attributes":{}},"environment":{}})).unwrap();
	let mut tx = f.store.pool.begin().await.unwrap();
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
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("sequence")),
			))
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("resource_id")),
			))
			.from(sea_orm::sea_query::Alias::new("authorization_decisions"))
			.and_where(sea_orm::sea_query::Expr::cust(
				"resource_id = 'cursor-test'",
			))
			.order_by_expr(
				sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
					sea_orm::sea_query::Alias::new("sequence"),
				)),
				sea_orm::sea_query::Order::Asc,
			)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_all(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(rows.len(), 2);
	assert!(rows[0].0 < rows[1].0);
	cleanup(f, &url, &schema).await;
}
