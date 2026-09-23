mod common;

use aidash::api;
use common::{cleanup, request, setup};
use serde_json::{Value, json};
use uuid::Uuid;

async fn workspace(app: &axum::Router, token: &str, title: &str) -> String {
	let (status, value) = request(app, token, "POST", "/api/workspaces", json!({"title":title,"goal":"Discuss and investigate"})).await;
	assert_eq!(status, 200, "{value}");
	value["id"].as_str().unwrap().to_owned()
}

async fn post(app: &axum::Router, token: &str, workspace: &str, content: &str, thread: Option<&str>, key: Uuid) -> (u16, Value) {
	request(app, token, "POST", &format!("/api/workspaces/{workspace}/thread-messages"), json!({"content":content,"thread_id":thread,"idempotency_key":key})).await
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
	let (status, thread) = request(&app, &token, "POST", &format!("/api/workspaces/{workspace}/threads"), json!({"root_message_id":root_id})).await;
	assert_eq!(status, 200, "{thread}");
	let thread_id = thread["id"].as_str().unwrap();
	let (status, reply) = post(&app, &token, &workspace, "Supporting information", Some(thread_id), Uuid::new_v4()).await;
	assert_eq!(status, 200, "{reply}");
	assert_eq!(reply["thread_id"], thread_id);
	let restarted = api::router(f.clone());
	let (status, history) = request(&restarted, &token, "GET", &format!("/api/workspaces/{workspace}/message-history?limit=20"), Value::Null).await;
	assert_eq!(status, 200, "{history}");
	assert_eq!(history["messages"].as_array().unwrap().len(), 1);
	assert_eq!(history["messages"][0]["message"]["content"], "A question");
	assert_eq!(history["messages"][0]["thread_id"], thread_id);
	let (status, replies) = request(&restarted, &token, "GET", &format!("/api/workspaces/{workspace}/message-history?thread_id={thread_id}&limit=20"), Value::Null).await;
	assert_eq!(status, 200, "{replies}");
	assert_eq!(replies["messages"].as_array().unwrap().len(), 2);
	assert_eq!(replies["messages"][1]["message"]["content"], "Supporting information");
	let (_, snapshot) = request(&restarted, &token, "GET", &format!("/api/workspaces/{workspace}"), Value::Null).await;
	assert_eq!(snapshot["tasks"].as_array().unwrap().len(), 0);
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
	assert_eq!(post(&app, &token, &workspace, "Changed", None, key).await.0, 409);
	let (_, thread) = request(&app, &token, "POST", &format!("/api/workspaces/{workspace}/threads"), json!({"root_message_id":first["message"]["id"]})).await;
	let thread_id = thread["id"].as_str().unwrap();
	assert_eq!(post(&app, &token, &workspace, "Original", Some(thread_id), key).await.0, 409);
	let (_, duplicate_thread) = request(&app, &token, "POST", &format!("/api/workspaces/{workspace}/threads"), json!({"root_message_id":first["message"]["id"]})).await;
	assert_eq!(thread["id"], duplicate_thread["id"]);
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
	let (_, thread) = request(&app, &token, "POST", &format!("/api/workspaces/{a}/threads"), json!({"root_message_id":root_id})).await;
	let thread_id = thread["id"].as_str().unwrap();
	assert_eq!(request(&app, &token, "POST", &format!("/api/workspaces/{b}/threads"), json!({"root_message_id":root_id})).await.0, 404);
	assert_eq!(post(&app, &token, &b, "Wrong channel", Some(thread_id), Uuid::new_v4()).await.0, 404);
	assert_eq!(request(&app, &token, "GET", &format!("/api/workspaces/{b}/message-history?thread_id={thread_id}"), Value::Null).await.0, 404);
	assert_eq!(request(&app, &token, "GET", &format!("/api/workspaces/{b}/message-history?before={root_id}"), Value::Null).await.0, 404);
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
	let (_, latest) = request(&app, &token, "GET", &format!("/api/workspaces/{workspace}/message-history?limit=2"), Value::Null).await;
	assert_eq!(latest["messages"].as_array().unwrap().len(), 2);
	assert_eq!(latest["messages"][0]["message"]["content"], "second");
	assert_eq!(latest["messages"][1]["message"]["content"], "third");
	let before = latest["next_before"].as_str().unwrap();
	let (_, older) = request(&app, &token, "GET", &format!("/api/workspaces/{workspace}/message-history?limit=2&before={before}"), Value::Null).await;
	assert_eq!(older["messages"].as_array().unwrap().len(), 1);
	assert_eq!(older["messages"][0]["message"]["content"], "first");
	assert!(older["next_before"].is_null());
	for query in ["limit=0", "limit=101"] {
		assert_eq!(request(&app, &token, "GET", &format!("/api/workspaces/{workspace}/message-history?{query}"), Value::Null).await.0, 400);
	}
	assert_eq!(post(&app, &token, &workspace, "   ", None, Uuid::new_v4()).await.0, 400);
	let spoof = request(&app, &token, "POST", &format!("/api/workspaces/{workspace}/thread-messages"), json!({"content":"spoof","sender":"admin","idempotency_key":Uuid::new_v4()})).await.0;
	assert_eq!(spoof, 422);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
async fn concurrent_identical_submissions_create_one_message() {
	let (f, url, schema) = setup().await;
	let token = f.config.api_token.clone();
	let app = api::router(f.clone());
	let workspace = workspace(&app, &token, "Concurrent").await;
	let key = Uuid::new_v4();
	let (a, b) = tokio::join!(post(&app, &token, &workspace, "one", None, key), post(&app, &token, &workspace, "one", None, key));
	assert_eq!(a.0, 200, "{}", a.1);
	assert_eq!(b.0, 200, "{}", b.1);
	assert_eq!(a.1["message"]["id"], b.1["message"]["id"]);
	let (_, history) = request(&app, &token, "GET", &format!("/api/workspaces/{workspace}/message-history"), Value::Null).await;
	assert_eq!(history["messages"].as_array().unwrap().len(), 1);
	cleanup(f, &url, &schema).await;
}
