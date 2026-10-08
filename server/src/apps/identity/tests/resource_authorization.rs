use common::upstream_fixtures;
use futures_util::{FutureExt, future::BoxFuture};
use reinhardt::ServerRouter as Router;
use std::sync::{
	Arc,
	atomic::{AtomicUsize, Ordering},
};
use upstream_fixtures::{async_upstream, handler};
#[path = "../../execution/tests/support/legacy.rs"]
mod common;
use aidash_server::{
	domain::{ArtifactInput, NewTask, qualified_agent},
	harness::Harness,
};
use common::*;
use serde_json::{Value, json};

#[rstest::rstest]
#[tokio::test]
async fn guarded_child_summary_pages_minimal_visible_rows(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
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
		.execute(f.store.pool.driver())
		.await
		.unwrap();
	let ids: Vec<uuid::Uuid> = sqlx::query_scalar(
		"SELECT id FROM tasks WHERE workspace_id = $1 AND parent_id = $2 ORDER BY id",
	)
	.bind(parent.workspace_id)
	.bind(parent_id)
	.fetch_all(f.store.pool.driver())
	.await
	.unwrap();
	let open_children = [ids[ids.len() - 2], ids[ids.len() - 1]];
	sqlx::query("UPDATE tasks SET status = 'OPEN' WHERE id = ANY($1)")
		.bind(open_children.as_slice())
		.execute(f.store.pool.driver())
		.await
		.unwrap();
	let authorization = aidash_server::authorization::Authorization {
		pool: f.store.pool.clone(),
	};
	let aidash_server::authorization::identity::Actor::Subject(identity) =
		authorization.authenticate(&token).await.unwrap()
	else {
		panic!("subject required")
	};
	let scope = aidash_server::authorization::workspace::Workspaces {
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
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
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
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
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
	assert_eq!(f.store.task(task).await.unwrap().status.as_str(), "OPEN");
	assert_eq!(f.store.tasks(Some(workspace)).await.unwrap().len(), 1);
	assert!(f.store.runs().await.unwrap().is_empty());
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn recorded_source_revocation_hides_journals_and_pauses_before_provider_io(
	#[future(awt)]
	#[from(retained_snapshot_revocation_provider)]
	fixture: RetainedSnapshotRevocationProvider,
) {
	retained_snapshot_revocation(fixture, false).await;
}

#[rstest::rstest]
#[tokio::test]
async fn workspace_event_revocation_hides_journals_and_pauses_before_provider_io(
	#[future(awt)]
	#[from(retained_snapshot_revocation_provider)]
	fixture: RetainedSnapshotRevocationProvider,
) {
	retained_snapshot_revocation(fixture, true).await;
}
async fn retained_snapshot_revocation(
	fixture: RetainedSnapshotRevocationProvider,
	events_only: bool,
) {
	let (f, url, schema) = fixture.application.runtime.parts();
	let app = fixture.application.application;
	let server = fixture.server;
	let endpoint = server.url.clone();
	let calls = fixture.calls;
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
		.execute(f.store.pool.driver())
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
	let authorization = aidash_server::authorization::Authorization {
		pool: f.store.pool.clone(),
	};
	let aidash_server::authorization::identity::Actor::Subject(identity) =
		authorization.authenticate(&token).await.unwrap()
	else {
		panic!("subject required")
	};
	let scope = aidash_server::authorization::workspace::Workspaces {
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
	assert_eq!(
		f.store.run(run.id).await.unwrap().control.as_str(),
		"PAUSED"
	);
	assert_eq!(calls.load(Ordering::SeqCst), 1);
	drop(server);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn opened_thread_events_retain_their_root_message_read_dependency(
	#[future(awt)]
	#[from(opened_thread_events_retain_their_root_message_read_dependency_provider)]
	fixture: OpenedThreadEventsRetainTheirRootMessageReadDependencyProvider,
) {
	let (f, url, schema) = fixture.application.runtime.parts();
	let app = fixture.application.application;
	let server = fixture.server;
	let endpoint = server.url.clone();
	let calls = fixture.calls;
	let (mut bundle, token, task) = bootstrap(&f, &app, &endpoint).await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	let root_content = "older thread root";
	f.store
		.message(workspace, "human", root_content, Some("thread-root-old"))
		.await
		.unwrap();
	let root: uuid::Uuid = {
		let query_bind_1 = workspace;
		let query_bind_2 = root_content;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.column(reinhardt::query::Alias::new("id"))
				.from(reinhardt::query::Alias::new("messages"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(workspace_id = ? AND content = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
	}
	.unwrap();
	let old = chrono::DateTime::parse_from_rfc3339("2000-01-01T00:00:00Z")
		.unwrap()
		.with_timezone(&chrono::Utc);
	{
		let query_bind_1 = old;
		let query_bind_2 = root;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("messages"))
				.value_expr(
					reinhardt::query::Alias::new("created_at"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					),
				)
				.and_where(
					reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
						reinhardt::query::Alias::new("id"),
					))
					.eq(SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					)),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
	}
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
	let recent: Vec<uuid::Uuid> = {
		let query_bind_1 = workspace;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.column(reinhardt::query::Alias::new("id"))
				.from(reinhardt::query::Alias::new("messages"))
				.and_where(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("workspace_id"))
						.eq(Expr::value(query_bind_1.to_owned())),
				)
				.order_by(
					reinhardt::query::Alias::new("created_at"),
					reinhardt::query::Order::Desc,
				)
				.order_by(
					reinhardt::query::Alias::new("id"),
					reinhardt::query::Order::Desc,
				)
				.limit(100)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_all(f.store.pool.driver())
		.await
	}
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
	let dependency: i64 = { let query_bind_1 = run.id; let query_bind_2 = workspace; let query_bind_3 = root; sqlx::query_scalar(&reinhardt::query::Query::select()
			.expr(reinhardt::query::Expr::cust("COUNT(*)"))
			.from(reinhardt::query::Alias::new("authorization_run_reads"))
			.and_where(SimpleExpr::CustomWithExpr("(run_id = ? AND workspace_id = ? AND resource_kind = 'message' AND resource_id = ?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into()]))
			.to_string(reinhardt::query::PostgresQueryBuilder))
	.fetch_one(f.store.pool.driver())
	.await }
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
	drop(server);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn stored_message_author_controls_visibility_and_forged_authorship_is_rejected(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
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
	let authorization = format!("Bearer {bob}");
	// reinhardt-web#6672: per-request headers append to defaults. Keep the shared
	// anonymous client free of default credentials so later Alice requests stay isolated.
	let rejected = app
		.client()
		.post_raw_with_headers(
			&path,
			json!({"content":"bob-private","sender":"alice"})
				.to_string()
				.as_bytes(),
			"application/json",
			&[("Authorization", authorization.as_str())],
		)
		.await
		.unwrap();
	assert_eq!(rejected.status_code(), 422);
	assert!(
		std::str::from_utf8(rejected.body())
			.unwrap()
			.contains("unknown field `sender`"),
		"forged authorship must retain the existing text rejection"
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
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
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
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::Expr::cust("COUNT(*)"))
			.from(reinhardt::query::Alias::new("authorization_decisions"))
			.and_where(reinhardt::query::Expr::cust(
				"action = 'task.read' AND decision ->> 'allowed' = 'false'",
			))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_one(f.store.pool.driver())
	.await
	.unwrap();
	assert!(denied > 0);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn cyclic_journal_dependencies_terminate_and_propagate_revocation(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (mut bundle, token, first) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let workspace = f.store.task(first).await.unwrap().workspace_id;
	let (_, second) = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{workspace}/tasks"),
		json!({"title":"Second","description":"Second journal"}),
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
	use aidash_server::apps::execution::models::Run as RunRecord;
	use aidash_server::apps::identity::models::{
		AuthorizationRunRead, states::AuthorizationRunReadResourceKind,
	};
	use reinhardt::db::orm::{DatabaseConnection, Json, Model};

	let mut db = *app.context.get_singleton::<DatabaseConnection>().unwrap();
	let journal = aidash_server::context::Context {
		history: vec![
			aidash_server::context::ContextEvent::ModelMediaObservation {
				text: "private journal content".into(),
				through_seq: None,
				truncated: false,
			},
		],
		..Default::default()
	};
	let mut updated = 0;
	for run in f.store.runs().await.unwrap() {
		let mut context = run.context.clone();
		context.history = journal.history.clone();
		updated += RunRecord::objects()
			.filter(RunRecord::field_id().eq(run.id))
			.update_fields_with_conn(
				&mut db,
				[(RunRecord::field_context(), Json(json!(context)))],
			)
			.await
			.unwrap();
	}
	assert_eq!(
		updated, 2,
		"both valid Run contexts retain the cyclic journal fixture"
	);
	let runs = f.store.runs().await.unwrap();
	assert_eq!(runs.len(), 2);
	// Build the cycle in the current schema. Existing-data migration is outside
	// this reset migration, but recursive visibility and revocation remain required.
	for (run, source) in [(runs[0].id, runs[1].id), (runs[1].id, runs[0].id)] {
		let read = AuthorizationRunRead::build()
			.run_id(run)
			.workspace_id(workspace)
			.resource_kind(AuthorizationRunReadResourceKind::Run)
			.resource_id(source)
			.finish();
		AuthorizationRunRead::objects()
			.create_with_conn(&mut db, &read)
			.await
			.unwrap();
	}
	assert_eq!(
		AuthorizationRunRead::objects()
			.filter(
				AuthorizationRunRead::field_resource_kind()
					.eq(AuthorizationRunReadResourceKind::Run)
			)
			.count_with_db(&mut db)
			.await
			.unwrap(),
		2
	);

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
	bundle["policies"].as_array_mut().unwrap().push(json!({"id":"cycle-source-revoked","effect":"deny","subjects":{"ids":["alice"]},"actions":["task.read"],"resources":{"kinds":["task"],"ids":[first]}}));
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
	#[from(worker_continues_with_visible_subset_and_never_sends_denied_records_provider)]
	fixture: WorkerContinuesWithVisibleSubsetAndNeverSendsDeniedRecordsProvider,
) {
	let (f, url, schema) = fixture.application.runtime.parts();
	let app = fixture.application.application;
	let server = fixture.server;
	let endpoint = server.url.clone();
	let calls = fixture.calls;
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
	assert_eq!(run.phase().as_str(), "COMPLETED");
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
	let hidden: i64 = {
		let query_bind_1 = artifact.id;
		let query_bind_2 = hidden_message;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust("COUNT(*)"))
				.from(reinhardt::query::Alias::new("authorization_run_reads"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(resource_id = ? OR resource_id = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
	}
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
	let recorded: i64 = { let query_bind_1 = run.id; let query_bind_2 = output.id; let query_bind_3 = message.id; sqlx::query_scalar(&reinhardt::query::Query::select().expr(reinhardt::query::Expr::cust("COUNT(*)")).from(reinhardt::query::Alias::new("authorization_run_reads")).and_where(SimpleExpr::CustomWithExpr("(run_id = ? AND ((resource_kind = 'artifact' AND resource_id = ?) OR (resource_kind = 'message' AND resource_id = ?)))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into()])).to_string(reinhardt::query::PostgresQueryBuilder))
    .fetch_one(f.store.pool.driver())
    .await }
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
	drop(server);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn artifact_state_page_is_filled_after_task_denials(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
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
	{
		let query_bind_1 = workspace;
		let query_bind_2 = hidden;
		sqlx::query(
			&reinhardt::query::Query::insert()
				.into_table(reinhardt::query::Alias::new("artifacts"))
				.columns([
					reinhardt::query::Alias::new("id"),
					reinhardt::query::Alias::new("workspace_id"),
					reinhardt::query::Alias::new("task_id"),
					reinhardt::query::Alias::new("kind"),
					reinhardt::query::Alias::new("name"),
					reinhardt::query::Alias::new("content"),
					reinhardt::query::Alias::new("created_by"),
					reinhardt::query::Alias::new("idempotency_key"),
					reinhardt::query::Alias::new("created_at"),
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
						.expr(reinhardt::query::Expr::cust("'text'"))
						.expr(reinhardt::query::Expr::cust("'Hidden'"))
						.expr(reinhardt::query::Expr::cust("'\"hidden\"'"))
						.expr(reinhardt::query::Expr::cust("'alice'"))
						.expr(reinhardt::query::Expr::cust("'hidden-' || i"))
						.expr(reinhardt::query::Expr::cust("CURRENT_TIMESTAMP"))
						.from_subquery(
							reinhardt::query::Query::select()
								.expr(reinhardt::query::Expr::cust("generate_series(1, 500)"))
								.to_owned(),
							reinhardt::query::Alias::new("i"),
						)
						.to_owned(),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
	}
	.unwrap();
	{
		let query_bind_1 = workspace;
		let query_bind_2 = visible;
		sqlx::query(
			&reinhardt::query::Query::insert()
				.into_table(reinhardt::query::Alias::new("artifacts"))
				.columns([
					reinhardt::query::Alias::new("id"),
					reinhardt::query::Alias::new("workspace_id"),
					reinhardt::query::Alias::new("task_id"),
					reinhardt::query::Alias::new("kind"),
					reinhardt::query::Alias::new("name"),
					reinhardt::query::Alias::new("content"),
					reinhardt::query::Alias::new("created_by"),
					reinhardt::query::Alias::new("idempotency_key"),
					reinhardt::query::Alias::new("created_at"),
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
						.expr(reinhardt::query::Expr::cust("'text'"))
						.expr(reinhardt::query::Expr::cust("'Visible'"))
						.expr(reinhardt::query::Expr::cust("'\"readable\"'"))
						.expr(reinhardt::query::Expr::cust("'alice'"))
						.expr(reinhardt::query::Expr::cust("'visible'"))
						.expr(reinhardt::query::Expr::cust(
							"CURRENT_TIMESTAMP - INTERVAL '1 DAY'",
						))
						.to_owned(),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
	}
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
	#[from(discovered_registry_entries_remain_live_journal_dependencies_provider)]
	fixture: DiscoveredRegistryEntriesRemainLiveJournalDependenciesProvider,
) {
	let (f, url, schema) = fixture.application.runtime.parts();
	let app = fixture.application.application;
	let server = fixture.server;
	let endpoint = server.url.clone();
	let _calls = fixture.calls;
	let (mut bundle, token, task) = bootstrap(&f, &app, &endpoint).await;
	let mut entry = f.registry.get("research", "1.0.0").await.unwrap();
	entry.id = "discovered-only".into();
	entry.binding_normalization = None;
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
	assert_eq!(run.phase().as_str(), "COMPLETED");
	let path = format!("/api/runs/{}", run.id);
	let (status, journal) = request(&app, &token, "GET", &path, Value::Null).await;
	assert_eq!(status, 200);
	assert!(journal.to_string().contains("discovered-private-metadata"));
	let tracked: i64 = {
		let query_bind_1 = run.id;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust("COUNT(*)"))
				.from(reinhardt::query::Alias::new(
					"authorization_run_registry_reads",
				))
				.and_where(SimpleExpr::CustomWithExpr(
					"(run_id = ? AND entry_id = 'discovered-only')".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(tracked, 1);
	// Act: rebuild the HTTP application over persisted reads before changing policy.
	drop(app);
	let app = common::application(f.clone()).await;
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
	drop(server);
	cleanup(f, &url, &schema).await;
}

use reinhardt::query::QueryStatementBuilder;

use reinhardt::query::ExprTrait;

use reinhardt::query::Expr;

use reinhardt::query::SimpleExpr;

#[rstest::fixture]
fn retained_snapshot_revocation_provider_calls() -> Arc<AtomicUsize> {
	Arc::new(AtomicUsize::new(0))
}

#[rstest::fixture]
fn retained_snapshot_revocation_provider_router(
	#[from(retained_snapshot_revocation_provider_calls)] calls: Arc<AtomicUsize>,
	runtime: common::RuntimeFuture,
) -> upstream_fixtures::RouterFuture {
	async move {


	let f = runtime.await.federation;

	let seen = calls.clone();
	let pool = f.store.pool.driver().clone();
	let server=Router::new().handler("/v1/chat/completions",handler(http::Method::POST, move |request: reinhardt::Request| {let body = request.json::<Value>().unwrap();
        let seen=seen.clone();let pool=pool.clone();async move {
            seen.fetch_add(1,Ordering::SeqCst);
            let context: Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
            let artifact_id = context["current"]["workspace"]["artifacts"][0]["id"].clone();
            assert!(artifact_id.is_string());
            assert!(!body.to_string().contains("private-artifact-content"));
            let sources:i64=sqlx::query_scalar(&reinhardt::query::Query::select().expr(reinhardt::query::Expr::cust("COUNT(*)")).from(reinhardt::query::Alias::new("authorization_run_reads")).and_where(reinhardt::query::Expr::cust("resource_kind = 'artifact'")).to_string(reinhardt::query::PostgresQueryBuilder)).fetch_one(&pool).await.unwrap();
            assert_eq!(sources,20,"only records exposed by the bounded observation page are tracked before provider I/O");
            reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"read","type":"function","function":{"name":"workspace_read","arguments":json!({"kind":"artifact","id":artifact_id}).to_string()}}]}}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
        }
    }));



Arc::new(server)}.boxed().shared()
}

struct RetainedSnapshotRevocationProvider {
	application: common::ApplicationFixture,
	server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	calls: Arc<AtomicUsize>,
}

#[rstest::fixture]
fn retained_snapshot_revocation_provider(
	#[from(retained_snapshot_revocation_provider_calls)] calls: Arc<AtomicUsize>,
	#[from(common::runtime)] _runtime: common::RuntimeFuture,
	#[from(retained_snapshot_revocation_provider_router)]
	#[with(calls.clone(), _runtime.clone())]
	_router: upstream_fixtures::RouterFuture,
	#[from(async_upstream)]
	#[with(_router.clone())]
	_server: upstream_fixtures::UpstreamFuture,
	#[from(common::native_application)]
	#[with(Default::default(),aidash_server::sse::Service::new(Default::default()),Arc::new(|r|r),_runtime.clone())]
	_application: common::ApplicationFuture,
) -> BoxFuture<'static, RetainedSnapshotRevocationProvider> {
	async move {
		RetainedSnapshotRevocationProvider {
			application: _application.await,
			server: _server.await,
			calls,
		}
	}
	.boxed()
}

#[rstest::fixture]
fn opened_thread_events_retain_their_root_message_read_dependency_provider_calls()
-> Arc<AtomicUsize> {
	Arc::new(AtomicUsize::new(0))
}

#[rstest::fixture]
fn opened_thread_events_retain_their_root_message_read_dependency_provider_router(
	#[from(opened_thread_events_retain_their_root_message_read_dependency_provider_calls)]
	calls: Arc<AtomicUsize>,
) -> upstream_fixtures::RouterFuture {
	async move {




	let seen = calls.clone();
	let server = Router::new().handler(
		"/v1/chat/completions",
		handler(http::Method::POST, move |request: reinhardt::Request| {let _body = request.json::<Value>().unwrap();
			let seen = seen.clone();
			async move {
				seen.fetch_add(1, Ordering::SeqCst);
				reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Noted the discussion."}}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
			}
		}),
	);



Arc::new(server)}.boxed().shared()
}

struct OpenedThreadEventsRetainTheirRootMessageReadDependencyProvider {
	application: common::ApplicationFixture,
	server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	calls: Arc<AtomicUsize>,
}

#[rstest::fixture]
fn opened_thread_events_retain_their_root_message_read_dependency_provider(
	#[from(opened_thread_events_retain_their_root_message_read_dependency_provider_calls)]
	calls: Arc<AtomicUsize>,
	#[from(common::runtime)] _runtime: common::RuntimeFuture,
	#[from(opened_thread_events_retain_their_root_message_read_dependency_provider_router)]
	#[with(calls.clone())]
	_router: upstream_fixtures::RouterFuture,
	#[from(async_upstream)]
	#[with(_router.clone())]
	_server: upstream_fixtures::UpstreamFuture,
	#[from(common::native_application)]
	#[with(Default::default(),aidash_server::sse::Service::new(Default::default()),Arc::new(|r|r),_runtime.clone())]
	_application: common::ApplicationFuture,
) -> BoxFuture<'static, OpenedThreadEventsRetainTheirRootMessageReadDependencyProvider> {
	async move {
		OpenedThreadEventsRetainTheirRootMessageReadDependencyProvider {
			application: _application.await,
			server: _server.await,
			calls,
		}
	}
	.boxed()
}

#[rstest::fixture]
fn worker_continues_with_visible_subset_and_never_sends_denied_records_provider_calls()
-> Arc<AtomicUsize> {
	Arc::new(AtomicUsize::new(0))
}

#[rstest::fixture]
fn worker_continues_with_visible_subset_and_never_sends_denied_records_provider_router(
	#[from(worker_continues_with_visible_subset_and_never_sends_denied_records_provider_calls)]
	calls: Arc<AtomicUsize>,
) -> upstream_fixtures::RouterFuture {
	async move {



	let seen = calls.clone();
	let server=Router::new().handler("/v1/chat/completions",handler(http::Method::POST, move |request: reinhardt::Request| {let body = request.json::<Value>().unwrap();let seen=seen.clone();async move {
        seen.fetch_add(1,Ordering::SeqCst);
        assert!(!body.to_string().contains("hidden-provider-source"));assert!(!body.to_string().contains("hidden-message"));
        assert!(body.to_string().contains("Use approved tools"));
        reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Visible work completed"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
    }}));



Arc::new(server)}.boxed().shared()
}

struct WorkerContinuesWithVisibleSubsetAndNeverSendsDeniedRecordsProvider {
	application: common::ApplicationFixture,
	server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	calls: Arc<AtomicUsize>,
}

#[rstest::fixture]
fn worker_continues_with_visible_subset_and_never_sends_denied_records_provider(
	#[from(worker_continues_with_visible_subset_and_never_sends_denied_records_provider_calls)]
	calls: Arc<AtomicUsize>,
	#[from(common::runtime)] _runtime: common::RuntimeFuture,
	#[from(worker_continues_with_visible_subset_and_never_sends_denied_records_provider_router)]
	#[with(calls.clone())]
	_router: upstream_fixtures::RouterFuture,
	#[from(async_upstream)]
	#[with(_router.clone())]
	_server: upstream_fixtures::UpstreamFuture,
	#[from(common::native_application)]
	#[with(Default::default(),aidash_server::sse::Service::new(Default::default()),Arc::new(|r|r),_runtime.clone())]
	_application: common::ApplicationFuture,
) -> BoxFuture<'static, WorkerContinuesWithVisibleSubsetAndNeverSendsDeniedRecordsProvider> {
	async move {
		WorkerContinuesWithVisibleSubsetAndNeverSendsDeniedRecordsProvider {
			application: _application.await,
			server: _server.await,
			calls,
		}
	}
	.boxed()
}

#[rstest::fixture]
fn discovered_registry_entries_remain_live_journal_dependencies_provider_calls() -> Arc<AtomicUsize>
{
	Arc::new(AtomicUsize::new(0))
}

#[rstest::fixture]
fn discovered_registry_entries_remain_live_journal_dependencies_provider_router(
	#[from(discovered_registry_entries_remain_live_journal_dependencies_provider_calls)] calls: Arc<
		AtomicUsize,
	>,
) -> upstream_fixtures::RouterFuture {
	async move {



	let server=Router::new().handler("/v1/chat/completions",handler(http::Method::POST, move |_request: reinhardt::Request| {let calls=calls.clone(); async move {
        let message=if calls.fetch_add(1,Ordering::SeqCst)==0 {
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"discover","type":"function","function":{"name":"agent_discover","arguments":"{}"}}]})
        } else { json!({"role":"assistant","content":"Completed discovery"}) };
        let reason=if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"};
        reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":reason,"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
    }}));



Arc::new(server)}.boxed().shared()
}

struct DiscoveredRegistryEntriesRemainLiveJournalDependenciesProvider {
	application: common::ApplicationFixture,
	server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
	calls: Arc<AtomicUsize>,
}

#[rstest::fixture]
fn discovered_registry_entries_remain_live_journal_dependencies_provider(
	#[from(discovered_registry_entries_remain_live_journal_dependencies_provider_calls)] calls: Arc<
		AtomicUsize,
	>,
	#[from(common::runtime)] _runtime: common::RuntimeFuture,
	#[from(discovered_registry_entries_remain_live_journal_dependencies_provider_router)]
	#[with(calls.clone())]
	_router: upstream_fixtures::RouterFuture,
	#[from(async_upstream)]
	#[with(_router.clone())]
	_server: upstream_fixtures::UpstreamFuture,
	#[from(common::native_application)]
	#[with(Default::default(),aidash_server::sse::Service::new(Default::default()),Arc::new(|r|r),_runtime.clone())]
	_application: common::ApplicationFuture,
) -> BoxFuture<'static, DiscoveredRegistryEntriesRemainLiveJournalDependenciesProvider> {
	async move {
		DiscoveredRegistryEntriesRemainLiveJournalDependenciesProvider {
			application: _application.await,
			server: _server.await,
			calls,
		}
	}
	.boxed()
}
