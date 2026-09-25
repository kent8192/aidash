mod common;
use aidash::{
	api,
	domain::{ArtifactInput, NewTask, qualified_agent},
	harness::Harness,
};
use common::*;
use common::{TestEnvironment, test_environment};
use serde_json::{Value, json};

#[rstest::rstest]
#[tokio::test]
async fn guarded_child_summary_pages_minimal_visible_rows(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (mut bundle, token, parent_id) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let parent = f.store.task(parent_id).await.unwrap();
	for index in 0..102 {
		f.store
			.create_task(
				parent.workspace_id,
				&NewTask {
					title: format!("Large child {index}"),
					description: "bounded summary payload ".repeat(2_500),
					requirements: json!({}),
					dependencies: vec![],
					parent_id: Some(parent_id),
				},
				"alice",
				None,
			)
			.await
			.unwrap();
	}
	sqlx::query("UPDATE tasks SET status = 'ABANDONED' WHERE workspace_id = $1 AND parent_id = $2")
		.bind(parent.workspace_id)
		.bind(parent_id)
		.execute(&f.store.pool)
		.await
		.unwrap();
	let ids: Vec<uuid::Uuid> = sqlx::query_scalar(
		"SELECT id FROM tasks WHERE workspace_id = $1 AND parent_id = $2 ORDER BY id",
	)
	.bind(parent.workspace_id)
	.bind(parent_id)
	.fetch_all(&f.store.pool)
	.await
	.unwrap();
	let open_children = [ids[ids.len() - 2], ids[ids.len() - 1]];
	sqlx::query("UPDATE tasks SET status = 'OPEN' WHERE id = ANY($1)")
		.bind(open_children.as_slice())
		.execute(&f.store.pool)
		.await
		.unwrap();
	let authorization = aidash::authorization::Authorization {
		pool: f.store.pool.clone(),
	};
	let aidash::authorization::identity::Actor::Subject(identity) =
		authorization.authenticate(&token).await.unwrap()
	else {
		panic!("subject required")
	};
	let scope = aidash::authorization::workspace::Workspaces {
		store: f.store.clone(),
		identity,
	};
	let summary = scope
		.child_task_summary(parent.workspace_id, parent_id)
		.await
		.unwrap();
	assert!(summary.has_pending && !summary.has_failed);

	for id in open_children {
		bundle["policies"].as_array_mut().unwrap().push(json!({
			"id":format!("deny-child-{id}"),
			"effect":"deny",
			"subjects":{"ids":["alice"]},
			"actions":["task.read"],
			"resources":{"kinds":["task"],"ids":[id]}
		}));
	}
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
	let summary = scope
		.child_task_summary(parent.workspace_id, parent_id)
		.await
		.unwrap();
	assert!(!summary.has_pending && !summary.has_failed);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn task_artifact_message_denials_filter_aggregate_events_and_run_details(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (mut bundle, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let task_record = f.store.task(task).await.unwrap();
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
	Harness {
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
			"resource-test-artifact",
			&ArtifactInput {
				kind: "text".into(),
				name: "Private report".into(),
				content: json!("private-artifact-content"),
			},
		)
		.await
		.unwrap();
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/workspaces/{}/messages", task_record.workspace_id),
			json!({"content":"private-message-content"})
		)
		.await
		.0,
		200
	);
	let (_, before) = request(
		&app,
		&token,
		"GET",
		&format!("/api/workspaces/{}", task_record.workspace_id),
		Value::Null,
	)
	.await;
	let message = before["messages"][0]["id"].as_str().unwrap();
	for (kind, id) in [
		("task", task.to_string()),
		("artifact", artifact.id.to_string()),
		("message", message.to_owned()),
	] {
		bundle["policies"].as_array_mut().unwrap().push(json!({"id":format!("deny-{kind}"),"effect":"deny","subjects":{"ids":["alice"]},"actions":[format!("{kind}.read")],"resources":{"kinds":[kind],"ids":[id]}}));
	}
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
	for path in [
		"/api/state".to_owned(),
		format!("/api/workspaces/{}", task_record.workspace_id),
		format!("/api/events?workspace_id={}", task_record.workspace_id),
	] {
		let (status, body) = request(&app, &token, "GET", &path, Value::Null).await;
		assert_eq!(status, 200, "{body}");
		assert!(
			!body.to_string().contains("private-artifact-content"),
			"artifact leaked through {path}"
		);
		assert!(
			!body.to_string().contains("private-message-content"),
			"message leaked through {path}"
		);
		assert!(
			!body.to_string().contains("Use approved tools"),
			"task leaked through {path}"
		);
	}
	let run = f.store.runs().await.unwrap().remove(0);
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
	let (_, snapshot) = request(
		&app,
		&token,
		"GET",
		&format!("/api/workspaces/{}", task_record.workspace_id),
		Value::Null,
	)
	.await;
	assert_eq!(snapshot["tasks"], json!([]));
	assert_eq!(snapshot["artifacts"], json!([]));
	assert_eq!(snapshot["messages"], json!([]));
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn hidden_task_cannot_be_claimed_or_used_as_a_dependency(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (mut bundle, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	bundle["policies"].as_array_mut().unwrap().push(json!({"id":"deny-task","effect":"deny","subjects":{"ids":["alice"]},"actions":["task.read"],"resources":{"kinds":["task"],"ids":[task]}}));
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
		403
	);
	for field in ["parent_id", "dependencies"] {
		let mut input = json!({"title":"Probe","description":"Hidden relationship"});
		input[field] = if field == "dependencies" {
			json!([task])
		} else {
			json!(task)
		};
		assert_eq!(
			request(
				&app,
				&token,
				"POST",
				&format!("/api/workspaces/{workspace}/tasks"),
				input
			)
			.await
			.0,
			403
		);
	}
	assert_eq!(f.store.task(task).await.unwrap().status, "OPEN");
	assert_eq!(f.store.tasks(Some(workspace)).await.unwrap().len(), 1);
	assert!(f.store.runs().await.unwrap().is_empty());
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn recorded_source_revocation_hides_journals_and_pauses_before_provider_io(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	retained_snapshot_revocation(&_test_environment, false).await;
}

#[rstest::rstest]
#[tokio::test]
async fn workspace_event_revocation_hides_journals_and_pauses_before_provider_io(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	retained_snapshot_revocation(&_test_environment, true).await;
}
async fn retained_snapshot_revocation(environment: &TestEnvironment, events_only: bool) {
	use axum::{Json, Router, routing::post};
	use std::sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	};
	let (f, url, schema) = setup(environment).await;
	let calls = Arc::new(AtomicUsize::new(0));
	let seen = calls.clone();
	let pool = f.store.pool.clone();
	let server=Router::new().route("/v1/chat/completions",post(move |Json(body):Json<Value>|{
        let seen=seen.clone();let pool=pool.clone();async move {
            seen.fetch_add(1,Ordering::SeqCst);
            let context: Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
            let artifact_id = context["current"]["workspace"]["artifacts"][0]["id"].clone();
            assert!(artifact_id.is_string());
            assert!(!body.to_string().contains("private-artifact-content"));
            let sources:i64=sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::Expr::cust("COUNT(*)")).from(sea_orm::sea_query::Alias::new("authorization_run_reads")).and_where(sea_orm::sea_query::Expr::cust("resource_kind = 'artifact'")).to_string(sea_orm::sea_query::PostgresQueryBuilder)).fetch_one(&pool).await.unwrap();
            assert_eq!(sources,20,"only records exposed by the bounded observation page are tracked before provider I/O");
            Json(json!({"choices":[{"index":0,"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"read","type":"function","function":{"name":"workspace_read","arguments":json!({"kind":"artifact","id":artifact_id}).to_string()}}]}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
        }
    }));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
	let app = api::router(f.clone());
	let (mut bundle, token, task) = bootstrap(&f, &app, &endpoint).await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
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
	let worker = Harness {
		federation: f.clone(),
	};
	worker.worker_once().await.unwrap();
	let artifact = f
		.store
		.publish_artifact(
			task,
			&qualified_agent(&f.config.node_id, "research", "1.0.0"),
			"source-revoke",
			&ArtifactInput {
				kind: "text".into(),
				name: "Source".into(),
				content: json!("private-artifact-content"),
			},
		)
		.await
		.unwrap();
	sqlx::query("UPDATE artifacts SET created_at = '2000-01-01T00:00:00Z' WHERE id = $1")
		.bind(artifact.id)
		.execute(&f.store.pool)
		.await
		.unwrap();
	for index in 0..25 {
		f.store
			.publish_artifact(
				task,
				&qualified_agent(&f.config.node_id, "research", "1.0.0"),
				&format!("unrelated-source-{index}"),
				&ArtifactInput {
					kind: "text".into(),
					name: format!("Unrelated source {index}"),
					content: json!("unrelated"),
				},
			)
			.await
			.unwrap();
	}
	worker.worker_once().await.unwrap();
	worker.worker_once().await.unwrap();
	let run = f.store.runs().await.unwrap().remove(0);
	assert!(run.context.to_string().contains("private-artifact-content"));
	let authorization = aidash::authorization::Authorization {
		pool: f.store.pool.clone(),
	};
	let aidash::authorization::identity::Actor::Subject(identity) =
		authorization.authenticate(&token).await.unwrap()
	else {
		panic!("subject required")
	};
	let scope = aidash::authorization::workspace::Workspaces {
		store: f.store.clone(),
		identity,
	};
	let buffered = scope.events(0, Some(workspace), 500).await.unwrap();
	let protected = buffered
		.iter()
		.find(|event| event.kind == "tool.completed")
		.unwrap();
	assert!(scope.can_emit(protected).await.unwrap());
	let denial = if events_only {
		json!({"id":"revoke-events","effect":"deny","subjects":{"ids":["alice"]},"actions":["workspace.events"],"resources":{"kinds":["workspace"],"ids":[workspace]}})
	} else {
		json!({"id":"revoke-source","effect":"deny","subjects":{"ids":["alice"]},"actions":["artifact.read"],"resources":{"kinds":["artifact"],"ids":[artifact.id]}})
	};
	bundle["policies"].as_array_mut().unwrap().push(denial);
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
	assert!(
		!scope.can_emit(protected).await.unwrap(),
		"buffered raw journals must be rechecked"
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
	if !events_only {
		for path in [
			"/api/state".to_owned(),
			format!("/api/workspaces/{workspace}"),
			format!("/api/events?workspace_id={workspace}"),
		] {
			let (status, body) = request(&app, &token, "GET", &path, Value::Null).await;
			assert_eq!(status, 200, "{body}");
			assert!(
				!body.to_string().contains("private-artifact-content"),
				"{path}"
			);
		}
	}
	worker.worker_once().await.unwrap();
	assert_eq!(f.store.run(run.id).await.unwrap().control, "PAUSED");
	assert_eq!(calls.load(Ordering::SeqCst), 1);
	server.abort();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn opened_thread_events_retain_their_root_message_read_dependency(
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
	let seen = calls.clone();
	let server = Router::new().route(
		"/v1/chat/completions",
		post(move |Json(_body): Json<Value>| {
			let seen = seen.clone();
			async move {
				seen.fetch_add(1, Ordering::SeqCst);
				Json(json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Noted the discussion."}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
			}
		}),
	);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (mut bundle, token, task) = bootstrap(&f, &app, &endpoint).await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	let root_content = "older thread root";
	f.store
		.message(workspace, "human", root_content, Some("thread-root-old"))
		.await
		.unwrap();
	let root: uuid::Uuid = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.column(sea_orm::sea_query::Alias::new("id"))
			.from(sea_orm::sea_query::Alias::new("messages"))
			.and_where(sea_orm::sea_query::Expr::cust(
				"workspace_id = $1 AND content = $2",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(workspace)
	.bind(root_content)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	let old = chrono::DateTime::parse_from_rfc3339("2000-01-01T00:00:00Z")
		.unwrap()
		.with_timezone(&chrono::Utc);
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("messages"))
			.value(
				sea_orm::sea_query::Alias::new("created_at"),
				sea_orm::sea_query::Expr::cust("$1"),
			)
			.and_where(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("id"))
					.eq(sea_orm::sea_query::Expr::cust("$2")),
			)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(old)
	.bind(root)
	.execute(&f.store.pool)
	.await
	.unwrap();
	for index in 0..110 {
		f.store
			.message(
				workspace,
				"human",
				&format!("newer message {index}"),
				Some(&format!("thread-burst-{index}")),
			)
			.await
			.unwrap();
	}
	let (status, thread) = request(
		&app,
		&f.config.api_token,
		"POST",
		&format!("/api/workspaces/{workspace}/threads"),
		json!({"root_message_id":root}),
	)
	.await;
	assert_eq!(status, 200, "{thread}");
	let recent: Vec<uuid::Uuid> = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.column(sea_orm::sea_query::Alias::new("id"))
			.from(sea_orm::sea_query::Alias::new("messages"))
			.and_where(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("workspace_id"))
					.eq(sea_orm::sea_query::Expr::cust("$1")),
			)
			.order_by(
				sea_orm::sea_query::Alias::new("created_at"),
				sea_orm::sea_query::Order::Desc,
			)
			.order_by(
				sea_orm::sea_query::Alias::new("id"),
				sea_orm::sea_query::Order::Desc,
			)
			.limit(100)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(workspace)
	.fetch_all(&f.store.pool)
	.await
	.unwrap();
	assert!(
		!recent.contains(&root),
		"the root is outside the message page"
	);

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
	let worker = Harness {
		federation: f.clone(),
	};
	for _ in 0..4 {
		worker.worker_once().await.unwrap();
		if calls.load(Ordering::SeqCst) > 0 {
			break;
		}
	}
	assert_eq!(calls.load(Ordering::SeqCst), 1);
	let run = f
		.store
		.runs()
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.task_id == task)
		.unwrap();
	let dependency: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
			.from(sea_orm::sea_query::Alias::new("authorization_run_reads"))
			.and_where(sea_orm::sea_query::Expr::cust(
				"run_id = $1 AND workspace_id = $2 AND resource_kind = 'message' AND resource_id = $3",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind(workspace)
	.bind(root)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(
		dependency, 1,
		"thread-open events depend on their root message"
	);

	bundle["policies"].as_array_mut().unwrap().push(json!({
		"id":"deny-thread-root", "effect":"deny", "subjects":{"ids":["alice"]},
		"actions":["message.read"], "resources":{"kinds":["message"],"ids":[root]}
	}));
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
	server.abort();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn stored_message_author_controls_visibility_and_forged_authorship_is_rejected(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (mut bundle, alice, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	bundle["subjects"]["bob"] = json!({"kind":"user"});
	bundle["policies"].as_array_mut().unwrap().push(json!({"id":"private-bob-message","effect":"deny","subjects":{"ids":["alice"]},"actions":["message.read"],"resources":{"kinds":["message"]},"condition":{"op":"eq","left":{"source":"resource","path":"/created_by"},"right":{"source":"literal","value":"bob"}}}));
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
	let (_, bob) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	let bob = bob["token"].as_str().unwrap();
	let path = format!("/api/workspaces/{workspace}/messages");
	assert_eq!(
		request(
			&app,
			bob,
			"POST",
			&path,
			json!({"content":"bob-private","sender":"alice"})
		)
		.await
		.0,
		422
	);
	assert_eq!(
		request(&app, bob, "POST", &path, json!({"content":"bob-private"}))
			.await
			.0,
		200
	);
	assert_eq!(
		request(
			&app,
			&alice,
			"POST",
			&path,
			json!({"content":"alice-visible"})
		)
		.await
		.0,
		200
	);
	let (_, snapshot) = request(
		&app,
		&alice,
		"GET",
		&format!("/api/workspaces/{workspace}"),
		Value::Null,
	)
	.await;
	assert!(snapshot.to_string().contains("alice-visible"));
	assert!(!snapshot.to_string().contains("bob-private"));
	assert_eq!(snapshot["messages"].as_array().unwrap().len(), 1);
	let (_, snapshot) = request(
		&app,
		bob,
		"GET",
		&format!("/api/workspaces/{workspace}"),
		Value::Null,
	)
	.await;
	assert!(snapshot.to_string().contains("bob-private"));
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn denied_new_task_read_rolls_back_creation_but_retains_the_decision(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (mut bundle, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	bundle["policies"].as_array_mut().unwrap().push(json!({"id":"deny-new-task","effect":"deny","subjects":{"ids":["alice"]},"actions":["task.read"],"resources":{"kinds":["task"]},"condition":{"op":"eq","left":{"source":"resource","path":"/created_by"},"right":{"source":"literal","value":"alice"}}}));
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
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/workspaces/{workspace}/tasks"),
			json!({"title":"Blocked new task","description":"Must roll back"})
		)
		.await
		.0,
		403
	);
	assert_eq!(f.store.tasks(Some(workspace)).await.unwrap().len(), 1);
	assert!(
		!f.store
			.events(0, Some(workspace), 100)
			.await
			.unwrap()
			.iter()
			.any(|e| e.data.to_string().contains("Blocked new task"))
	);
	let denied: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
			.from(sea_orm::sea_query::Alias::new("authorization_decisions"))
			.and_where(sea_orm::sea_query::Expr::cust(
				"action = 'task.read' AND decision ->> 'allowed' = 'false'",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert!(denied > 0);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn legacy_journal_migration_retains_sources_and_cyclic_read_graphs_terminate(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (mut bundle, token, first) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let workspace = f.store.task(first).await.unwrap().workspace_id;
	let (_, second) = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{workspace}/tasks"),
		json!({"title":"Second","description":"Second old journal"}),
	)
	.await;
	for id in [first.to_string(), second["id"].as_str().unwrap().into()] {
		assert_eq!(
			request(
				&app,
				&token,
				"POST",
				&format!("/api/tasks/{id}/claim"),
				json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}})
			)
			.await
			.0,
			200
		);
	}
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("runs"))
			.value(
				sea_orm::sea_query::Alias::new("context"),
				sea_orm::sea_query::Expr::cust("$1"),
			)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(json!({"history":[{"kind":"assistant","text":"old private content"}]}))
	.execute(&f.store.pool)
	.await
	.unwrap();
	// Exercise the actual upgrade against populated old journals in this test's
	// isolated schema; no production table or other fixture is modified.
	sqlx::query(
		&sea_orm::sea_query::Table::drop()
			.table(sea_orm::sea_query::Alias::new("authorization_run_reads"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.execute(&f.store.pool)
	.await
	.unwrap();
	sqlx::query(
		&sea_orm::sea_query::Query::delete()
			.from_table(sea_orm::sea_query::Alias::new("seaql_migrations"))
			.and_where(sea_orm::sea_query::Expr::cust(
				"version LIKE '%m0011_resource_reads'",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.execute(&f.store.pool)
	.await
	.unwrap();
	aidash::store::Store::migrate(&f.store.pool).await.unwrap();
	let sources: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
			.from(sea_orm::sea_query::Alias::new("authorization_run_reads"))
			.and_where(sea_orm::sea_query::Expr::cust("resource_kind = 'run'"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(sources, 2);
	let runs = f.store.runs().await.unwrap();
	for run in &runs {
		assert_eq!(
			tokio::time::timeout(
				std::time::Duration::from_secs(3),
				request(
					&app,
					&token,
					"GET",
					&format!("/api/runs/{}", run.id),
					Value::Null
				)
			)
			.await
			.unwrap()
			.0,
			200
		);
	}
	bundle["policies"].as_array_mut().unwrap().push(json!({"id":"legacy-source-revoked","effect":"deny","subjects":{"ids":["alice"]},"actions":["task.read"],"resources":{"kinds":["task"],"ids":[first]}}));
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
	for run in &runs {
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
	}
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn worker_continues_with_visible_subset_and_never_sends_denied_records(
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
	let seen = calls.clone();
	let server=Router::new().route("/v1/chat/completions",post(move |Json(body):Json<Value>|{let seen=seen.clone();async move {
        seen.fetch_add(1,Ordering::SeqCst);
        assert!(!body.to_string().contains("hidden-provider-source"));assert!(!body.to_string().contains("hidden-message"));
        assert!(body.to_string().contains("Use approved tools"));
        Json(json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Visible work completed"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
    }}));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (mut bundle, token, task) = bootstrap(&f, &app, &endpoint).await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
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
	let worker = Harness {
		federation: f.clone(),
	};
	worker.worker_once().await.unwrap();
	let artifact = f
		.store
		.publish_artifact(
			task,
			&qualified_agent(&f.config.node_id, "research", "1.0.0"),
			"hidden-provider-source",
			&ArtifactInput {
				kind: "text".into(),
				name: "Hidden".into(),
				content: json!("hidden-provider-source"),
			},
		)
		.await
		.unwrap();
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/workspaces/{workspace}/messages"),
			json!({"content":"hidden-message"})
		)
		.await
		.0,
		200
	);
	let hidden_message = f.store.snapshot(workspace).await.unwrap().messages[0].id;
	bundle["policies"].as_array_mut().unwrap().push(json!({"id":"hide-artifact","effect":"deny","subjects":{"ids":["alice"]},"actions":["artifact.read"],"resources":{"kinds":["artifact"],"ids":[artifact.id]}}));
	bundle["policies"].as_array_mut().unwrap().push(json!({"id":"hide-messages","effect":"deny","subjects":{"ids":["alice"]},"actions":["message.read"],"resources":{"kinds":["message"],"ids":[hidden_message]}}));
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
	for _ in 0..6 {
		worker.worker_once().await.unwrap();
	}
	let run = f.store.runs().await.unwrap().remove(0);
	assert_eq!(run.phase, "COMPLETED");
	assert_eq!(calls.load(Ordering::SeqCst), 1);
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
		200
	);
	let hidden: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
			.from(sea_orm::sea_query::Alias::new("authorization_run_reads"))
			.and_where(sea_orm::sea_query::Expr::cust(
				"resource_id = $1 OR resource_id = $2",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(artifact.id)
	.bind(hidden_message)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(hidden, 0);
	let snapshot = f.store.snapshot(workspace).await.unwrap();
	let output = snapshot
		.artifacts
		.iter()
		.find(|a| a.content == json!("Visible work completed"))
		.unwrap();
	let message = snapshot
		.messages
		.iter()
		.find(|m| m.content == "Visible work completed")
		.unwrap();
	let recorded: i64 = sqlx::query_scalar(
        &sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::Expr::cust("COUNT(*)")).from(sea_orm::sea_query::Alias::new("authorization_run_reads")).and_where(sea_orm::sea_query::Expr::cust("run_id = $1 AND ((resource_kind = 'artifact' AND resource_id = $2) OR (resource_kind = 'message' AND resource_id = $3))")).to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .bind(run.id)
    .bind(output.id)
    .bind(message.id)
    .fetch_one(&f.store.pool)
    .await
    .unwrap();
	assert_eq!(
		recorded, 2,
		"producer outputs retain their read requirements"
	);
	// Each output is independently authorized, but its raw producer journal
	// contains both. Denying either resource must conceal that journal.
	for (index, (kind, id)) in [("artifact", output.id), ("message", message.id)]
		.into_iter()
		.enumerate()
	{
		let mut changed = bundle.clone();
		changed["policies"].as_array_mut().unwrap().push(json!({"id":"deny-produced-output","effect":"deny","subjects":{"ids":["alice"]},"actions":[format!("{kind}.read")],"resources":{"kinds":[kind],"ids":[id]}}));
		assert_eq!(
			request(
				&app,
				&f.config.api_token,
				"POST",
				"/api/authorization/acme",
				json!({"expected_revision":2+index,"bundle":changed})
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
		let (status, state) = request(&app, &token, "GET", "/api/state", Value::Null).await;
		assert_eq!(status, 200);
		assert!(state["runs"].as_array().unwrap().is_empty());
	}
	server.abort();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn artifact_state_page_is_filled_after_task_denials(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (mut bundle, token, hidden) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let workspace = f.store.task(hidden).await.unwrap().workspace_id;
	let (_, visible) = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{workspace}/tasks"),
		json!({"title":"Visible","description":"An older readable artifact"}),
	)
	.await;
	let visible: uuid::Uuid = visible["id"].as_str().unwrap().parse().unwrap();
	sqlx::query(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new("artifacts"))
			.columns([
				sea_orm::sea_query::Alias::new("id"),
				sea_orm::sea_query::Alias::new("workspace_id"),
				sea_orm::sea_query::Alias::new("task_id"),
				sea_orm::sea_query::Alias::new("kind"),
				sea_orm::sea_query::Alias::new("name"),
				sea_orm::sea_query::Alias::new("content"),
				sea_orm::sea_query::Alias::new("created_by"),
				sea_orm::sea_query::Alias::new("idempotency_key"),
				sea_orm::sea_query::Alias::new("created_at"),
			])
			.select_from(
				sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::Expr::cust("GEN_RANDOM_UUID()"))
					.expr(sea_orm::sea_query::Expr::cust("$1"))
					.expr(sea_orm::sea_query::Expr::cust("$2"))
					.expr(sea_orm::sea_query::Expr::cust("'text'"))
					.expr(sea_orm::sea_query::Expr::cust("'Hidden'"))
					.expr(sea_orm::sea_query::Expr::cust("'\"hidden\"'"))
					.expr(sea_orm::sea_query::Expr::cust("'alice'"))
					.expr(sea_orm::sea_query::Expr::cust("'hidden-' || i"))
					.expr(sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP"))
					.from_function(
						sea_orm::sea_query::Func::cust(sea_orm::sea_query::Alias::new(
							"generate_series",
						))
						.args([
							sea_orm::sea_query::Expr::val(1).into(),
							sea_orm::sea_query::Expr::val(500).into(),
						]),
						sea_orm::sea_query::Alias::new("i"),
					)
					.to_owned(),
			)
			.expect("valid insert projection")
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(workspace)
	.bind(hidden)
	.execute(&f.store.pool)
	.await
	.unwrap();
	sqlx::query(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new("artifacts"))
			.columns([
				sea_orm::sea_query::Alias::new("id"),
				sea_orm::sea_query::Alias::new("workspace_id"),
				sea_orm::sea_query::Alias::new("task_id"),
				sea_orm::sea_query::Alias::new("kind"),
				sea_orm::sea_query::Alias::new("name"),
				sea_orm::sea_query::Alias::new("content"),
				sea_orm::sea_query::Alias::new("created_by"),
				sea_orm::sea_query::Alias::new("idempotency_key"),
				sea_orm::sea_query::Alias::new("created_at"),
			])
			.values_panic([
				sea_orm::sea_query::Expr::cust("GEN_RANDOM_UUID()"),
				sea_orm::sea_query::Expr::cust("$1"),
				sea_orm::sea_query::Expr::cust("$2"),
				sea_orm::sea_query::Expr::cust("'text'"),
				sea_orm::sea_query::Expr::cust("'Visible'"),
				sea_orm::sea_query::Expr::cust("'\"readable\"'"),
				sea_orm::sea_query::Expr::cust("'alice'"),
				sea_orm::sea_query::Expr::cust("'visible'"),
				sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 DAY'"),
			])
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(workspace)
	.bind(visible)
	.execute(&f.store.pool)
	.await
	.unwrap();
	bundle["policies"].as_array_mut().unwrap().push(json!({"id":"hide-task","effect":"deny","subjects":{"ids":["alice"]},"actions":["task.read"],"resources":{"kinds":["task"],"ids":[hidden]}}));
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
	let (status, state) = request(&app, &token, "GET", "/api/state", Value::Null).await;
	assert_eq!(status, 200);
	assert_eq!(state["artifacts"].as_array().unwrap().len(), 1);
	assert_eq!(state["artifacts"][0]["content"], "readable");
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn discovered_registry_entries_remain_live_journal_dependencies(
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
	let server=Router::new().route("/v1/chat/completions",post(move || {let calls=calls.clone(); async move {
        let message=if calls.fetch_add(1,Ordering::SeqCst)==0 {
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"discover","type":"function","function":{"name":"agent_discover","arguments":"{}"}}]})
        } else { json!({"role":"assistant","content":"Completed discovery"}) };
        let reason=if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"};
        Json(json!({"choices":[{"index":0,"finish_reason":reason,"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
    }}));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
	let (f, url, schema) = setup(&_test_environment).await;
	let app = api::router(f.clone());
	let (mut bundle, token, task) = bootstrap(&f, &app, &endpoint).await;
	let mut entry = f.registry.get("research", "1.0.0").await.unwrap();
	entry.id = "discovered-only".into();
	entry
		.description
		.insert("en".into(), "discovered-private-metadata".into());
	f.registry.register(entry).await.unwrap();
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"discovered-only","version":"1.0.0"},"expected_revision":0,"enabled":true})
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
			&format!("/api/tasks/{task}/claim"),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}})
		)
		.await
		.0,
		200
	);
	let worker = Harness {
		federation: f.clone(),
	};
	for _ in 0..12 {
		worker.worker_once().await.unwrap();
	}
	let run = f.store.runs().await.unwrap().remove(0);
	assert_eq!(run.phase, "COMPLETED");
	let path = format!("/api/runs/{}", run.id);
	let (status, journal) = request(&app, &token, "GET", &path, Value::Null).await;
	assert_eq!(status, 200);
	assert!(journal.to_string().contains("discovered-private-metadata"));
	let tracked: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
			.from(sea_orm::sea_query::Alias::new(
				"authorization_run_registry_reads",
			))
			.and_where(sea_orm::sea_query::Expr::cust(
				"run_id = $1 AND entry_id = 'discovered-only'",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(run.id)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(tracked, 1);
	// Verify an existing journal is protected when upgrading from the previous schema.
	let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(f.store.pool.clone());
	use migration::MigratorTrait;
	migration::Migrator::down(&db, Some(1)).await.unwrap();
	migration::Migrator::up(&db, None).await.unwrap();
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"discovered-only","version":"1.0.0"},"expected_revision":1,"enabled":false})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(&app, &token, "GET", &path, Value::Null).await.0,
		403
	);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"discovered-only","version":"1.0.0"},"expected_revision":2,"enabled":true})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(&app, &token, "GET", &path, Value::Null).await.0,
		200
	);
	bundle["policies"].as_array_mut().unwrap().push(json!({"id":"hide-discovered-registry","effect":"deny","subjects":{"ids":["alice"]},"actions":["registry.read"],"resources":{"kinds":["agent"],"ids":["discovered-only"]}}));
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
	assert_eq!(
		request(&app, &token, "GET", &path, Value::Null).await.0,
		403
	);
	server.abort();
	cleanup(f, &url, &schema).await;
}
