mod common;

use aidash::api;
use axum::Router;
use common::{bootstrap, cleanup, request, setup};
use serde_json::{Value, json};
use uuid::Uuid;

async fn workspace(app: &Router, token: &str, title: &str) -> String {
	let (status, value) = request(
		app,
		token,
		"POST",
		"/api/workspaces",
		json!({"title":title,"goal":"Discuss and investigate"}),
	)
	.await;
	assert_eq!(status, 200, "{value}");
	value["id"].as_str().unwrap().to_owned()
}

async fn post(
	app: &Router,
	token: &str,
	workspace: &str,
	content: &str,
	thread: Option<&str>,
	key: Uuid,
) -> (u16, Value) {
	request(
		app,
		token,
		"POST",
		&format!("/api/workspaces/{workspace}/thread-messages"),
		json!({"content":content,"thread_id":thread,"idempotency_key":key}),
	)
	.await
}

async fn thread(app: &Router, token: &str, workspace: &str, root: &str) -> (u16, Value) {
	request(
		app,
		token,
		"POST",
		&format!("/api/workspaces/{workspace}/threads"),
		json!({"root_message_id":root}),
	)
	.await
}

async fn history(app: &Router, token: &str, workspace: &str, query: &str) -> (u16, Value) {
	request(
		app,
		token,
		"GET",
		&format!("/api/workspaces/{workspace}/message-history?{query}"),
		Value::Null,
	)
	.await
}

#[tokio::test]
async fn channel_threads_survive_new_router_and_do_not_become_tasks() {
	let (f, url, schema) = setup().await;
	let token = f.config.api_token.clone();
	let app = api::router(f.clone());
	let workspace = workspace(&app, &token, "Threads").await;
	let (status, root) = post(&app, &token, &workspace, "A question", None, Uuid::new_v4()).await;
	assert_eq!(status, 200, "{root}");
	let root_id = root["message"]["id"].as_str().unwrap();
	let (status, opened) = thread(&app, &token, &workspace, root_id).await;
	assert_eq!(status, 200, "{opened}");
	let thread_id = opened["id"].as_str().unwrap();
	let (status, reply) = post(
		&app,
		&token,
		&workspace,
		"Supporting information",
		Some(thread_id),
		Uuid::new_v4(),
	)
	.await;
	assert_eq!(status, 200, "{reply}");
	assert_eq!(reply["thread_id"], thread_id);
	let restarted = api::router(f.clone());
	let (status, channel) = history(&restarted, &token, &workspace, "limit=20").await;
	assert_eq!(status, 200, "{channel}");
	assert_eq!(channel["messages"].as_array().unwrap().len(), 1);
	assert_eq!(channel["messages"][0]["message"]["content"], "A question");
	assert_eq!(channel["messages"][0]["thread_id"], thread_id);
	let (status, replies) = history(
		&restarted,
		&token,
		&workspace,
		&format!("thread_id={thread_id}&limit=20"),
	)
	.await;
	assert_eq!(status, 200, "{replies}");
	assert_eq!(replies["messages"].as_array().unwrap().len(), 2);
	assert_eq!(
		replies["messages"][1]["message"]["content"],
		"Supporting information"
	);
	let snapshot = f.store.snapshot(workspace.parse().unwrap()).await.unwrap();
	assert!(snapshot.tasks.is_empty());
	assert!(
		snapshot
			.events
			.iter()
			.any(|event| event.kind == "message.thread_opened")
	);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
async fn duplicate_message_reuses_id_but_changed_thread_or_content_conflicts() {
	let (f, url, schema) = setup().await;
	let token = f.config.api_token.clone();
	let app = api::router(f.clone());
	let workspace = workspace(&app, &token, "Idempotency").await;
	let key = Uuid::new_v4();
	let (status, first) = post(&app, &token, &workspace, "Original", None, key).await;
	assert_eq!(status, 200, "{first}");
	let (status, second) = post(&app, &token, &workspace, "Original", None, key).await;
	assert_eq!(status, 200, "{second}");
	assert_eq!(first["message"]["id"], second["message"]["id"]);
	assert_eq!(
		post(&app, &token, &workspace, "Changed", None, key).await.0,
		409
	);
	let root = first["message"]["id"].as_str().unwrap();
	let (_, opened) = thread(&app, &token, &workspace, root).await;
	let thread_id = opened["id"].as_str().unwrap();
	let conflict = post(&app, &token, &workspace, "Original", Some(thread_id), key).await;
	assert_eq!(conflict.0, 409);
	let (_, duplicate) = thread(&app, &token, &workspace, root).await;
	assert_eq!(opened["id"], duplicate["id"]);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
async fn threads_and_cursors_cannot_cross_workspace_boundaries() {
	let (f, url, schema) = setup().await;
	let token = f.config.api_token.clone();
	let app = api::router(f.clone());
	let a = workspace(&app, &token, "A").await;
	let b = workspace(&app, &token, "B").await;
	let (status, root) = post(&app, &token, &a, "Private to A", None, Uuid::new_v4()).await;
	assert_eq!(status, 200, "{root}");
	let root_id = root["message"]["id"].as_str().unwrap();
	let (_, opened) = thread(&app, &token, &a, root_id).await;
	let thread_id = opened["id"].as_str().unwrap();
	assert_eq!(thread(&app, &token, &b, root_id).await.0, 404);
	let wrong = post(
		&app,
		&token,
		&b,
		"Wrong channel",
		Some(thread_id),
		Uuid::new_v4(),
	)
	.await;
	assert_eq!(wrong.0, 404);
	let query = format!("thread_id={thread_id}");
	assert_eq!(history(&app, &token, &b, &query).await.0, 404);
	let query = format!("before={root_id}");
	assert_eq!(history(&app, &token, &b, &query).await.0, 404);
	let (_, reply) = post(&app, &token, &a, "Reply", Some(thread_id), Uuid::new_v4()).await;
	let reply_id = reply["message"]["id"].as_str().unwrap();
	assert_eq!(thread(&app, &token, &a, reply_id).await.0, 409);
	let query = format!("before={reply_id}");
	assert_eq!(history(&app, &token, &a, &query).await.0, 404);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
async fn message_history_pages_without_duplicates_and_validates_inputs() {
	let (f, url, schema) = setup().await;
	let token = f.config.api_token.clone();
	let app = api::router(f.clone());
	let workspace = workspace(&app, &token, "Paging").await;
	for content in ["first", "second", "third"] {
		let (status, value) = post(&app, &token, &workspace, content, None, Uuid::new_v4()).await;
		assert_eq!(status, 200, "{value}");
	}
	let (_, latest) = history(&app, &token, &workspace, "limit=2").await;
	assert_eq!(latest["messages"].as_array().unwrap().len(), 2);
	assert_eq!(latest["messages"][0]["message"]["content"], "second");
	assert_eq!(latest["messages"][1]["message"]["content"], "third");
	let before = latest["next_before"].as_str().unwrap();
	let query = format!("limit=2&before={before}");
	let (_, older) = history(&app, &token, &workspace, &query).await;
	assert_eq!(older["messages"].as_array().unwrap().len(), 1);
	assert_eq!(older["messages"][0]["message"]["content"], "first");
	assert!(older["next_before"].is_null());
	for query in ["limit=0", "limit=101"] {
		assert_eq!(history(&app, &token, &workspace, query).await.0, 400);
	}
	let blank = post(&app, &token, &workspace, "   ", None, Uuid::new_v4()).await;
	assert_eq!(blank.0, 400);
	let spoof = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{workspace}/thread-messages"),
		json!({"content":"spoof","sender":"admin","idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(spoof.0, 422);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
async fn concurrent_identical_submissions_create_one_message() {
	let (f, url, schema) = setup().await;
	let token = f.config.api_token.clone();
	let app = api::router(f.clone());
	let workspace = workspace(&app, &token, "Concurrent").await;
	let key = Uuid::new_v4();
	let (a, b) = tokio::join!(
		post(&app, &token, &workspace, "one", None, key),
		post(&app, &token, &workspace, "one", None, key)
	);
	assert_eq!(a.0, 200, "{}", a.1);
	assert_eq!(b.0, 200, "{}", b.1);
	assert_eq!(a.1["message"]["id"], b.1["message"]["id"]);
	let (_, page) = history(&app, &token, &workspace, "").await;
	assert_eq!(page["messages"].as_array().unwrap().len(), 1);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
async fn scoped_history_filters_records_and_rechecks_root_and_post_authority() {
	let (f, url, schema) = setup().await;
	let operator = f.config.api_token.clone();
	let app = api::router(f.clone());
	let foreign = workspace(&app, &operator, "Operator-owned").await;
	let (mut policy, alice, task) = bootstrap(&f, &app, "http://127.0.0.1:1").await;
	let workspace = f.store.task(task).await.unwrap().workspace_id.to_string();
	let mut ids = Vec::new();
	for text in ["first", "hidden", "last"] {
		let (status, value) = post(&app, &alice, &workspace, text, None, Uuid::new_v4()).await;
		assert_eq!(status, 200, "{value}");
		assert_eq!(value["message"]["sender"], "alice");
		ids.push(value["message"]["id"].as_str().unwrap().to_owned());
	}
	let (_, opened) = thread(&app, &alice, &workspace, &ids[0]).await;
	let thread_id = opened["id"].as_str().unwrap();
	let (_, reply) = post(
		&app,
		&alice,
		&workspace,
		"nested",
		Some(thread_id),
		Uuid::new_v4(),
	)
	.await;
	assert!(reply["message"]["id"].is_string());
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"hide-one", "effect":"deny", "subjects":{"any":true},
		"actions":["message.read"], "resources":{"kinds":["message"],"ids":[ids[1]]}
	}));
	let updated = request(
		&app,
		&operator,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
	)
	.await;
	assert_eq!(updated.0, 200, "{}", updated.1);
	let (status, page) = history(&app, &alice, &workspace, "limit=1").await;
	assert_eq!(status, 200, "{page}");
	assert_eq!(page["messages"][0]["message"]["content"], "last");
	assert_eq!(page["next_before"], ids[2]);
	let query = format!("limit=1&before={}", ids[2]);
	let (status, older) = history(&app, &alice, &workspace, &query).await;
	assert_eq!(status, 200, "{older}");
	assert_eq!(older["messages"][0]["message"]["content"], "first");
	assert!(older["next_before"].is_null());
	assert!(!older.to_string().contains("hidden"));
	let query = format!("before={}", ids[1]);
	assert_eq!(history(&app, &alice, &workspace, &query).await.0, 404);
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"hide-root", "effect":"deny", "subjects":{"any":true},
		"actions":["message.read"], "resources":{"kinds":["message"],"ids":[ids[0]]}
	}));
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"read-only", "effect":"deny", "subjects":{"any":true},
		"actions":["message.create"], "resources":{"kinds":["workspace"]}
	}));
	let updated = request(
		&app,
		&operator,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":policy}),
	)
	.await;
	assert_eq!(updated.0, 200, "{}", updated.1);
	let query = format!("thread_id={thread_id}");
	assert_eq!(history(&app, &alice, &workspace, &query).await.0, 404);
	let denied = post(&app, &alice, &workspace, "denied", None, Uuid::new_v4()).await;
	assert_eq!(denied.0, 403);
	assert_eq!(thread(&app, &alice, &workspace, &ids[2]).await.0, 403);
	assert_eq!(history(&app, &alice, &foreign, "").await.0, 404);
	cleanup(f, &url, &schema).await;
}
