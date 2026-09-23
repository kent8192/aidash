mod common;

use aidash::api;
use axum::{Router, body::Body, http::Request};
use common::{bootstrap, cleanup, request, setup};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

async fn upload(app: &Router, token: &str, workspace: &str, key: Uuid, bytes: &[u8]) -> (u16, Value) {
	let path = format!(
		"/api/workspaces/{workspace}/attachments?filename=evidence.txt&media_type=text%2Fplain&idempotency_key={key}"
	);
	let response = app
		.clone()
		.oneshot(
			Request::post(path)
				.header("authorization", format!("Bearer {token}"))
				.header("content-type", "application/octet-stream")
				.body(Body::from(bytes.to_vec()))
				.unwrap(),
		)
		.await
		.unwrap();
	let status = response.status().as_u16();
	let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
	let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
	(status, value)
}

#[tokio::test]
async fn attachment_upload_is_idempotent_and_download_requires_current_message_access() {
	let (f, url, schema) = setup().await;
	let operator = f.config.api_token.clone();
	let app = api::router(f.clone());
	let (mut policy, alice, task) = bootstrap(&f, &app, "http://127.0.0.1:1").await;
	let workspace = f.store.task(task).await.unwrap().workspace_id.to_string();
	let key = Uuid::new_v4();
	let (status, file) = upload(&app, &alice, &workspace, key, b"Source evidence").await;
	assert_eq!(status, 200, "{file}");
	let (status, replay) = upload(&app, &alice, &workspace, key, b"Source evidence").await;
	assert_eq!(status, 200, "{replay}");
	assert_eq!(file["id"], replay["id"]);
	assert_eq!(upload(&app, &alice, &workspace, key, b"Changed bytes").await.0, 409);
	let attachment = file["id"].as_str().unwrap();
	let message_key = Uuid::new_v4();
	let message_path = format!("/api/workspaces/{workspace}/thread-messages");
	let body = json!({
		"content":"Review this source", "thread_id":null,
		"idempotency_key":message_key, "attachment_ids":[attachment]
	});
	let (status, message) = request(&app, &alice, "POST", &message_path, body.clone()).await;
	assert_eq!(status, 200, "{message}");
	assert_eq!(message["attachments"][0]["filename"], "evidence.txt");
	assert_eq!(message["attachments"][0]["size_bytes"], 15);
	let (status, replay) = request(&app, &alice, "POST", &message_path, body).await;
	assert_eq!(status, 200, "{replay}");
	assert_eq!(message["message"]["id"], replay["message"]["id"]);
	let download = format!("/api/workspaces/{workspace}/attachments/{attachment}");
	let response = app.clone().oneshot(
		Request::get(&download)
			.header("authorization", format!("Bearer {alice}"))
			.body(Body::empty()).unwrap()
	).await.unwrap();
	assert_eq!(response.status(), 200);
	assert_eq!(response.headers()["x-content-type-options"], "nosniff");
	assert!(response.headers()["content-disposition"].to_str().unwrap().starts_with("attachment;"));
	let bytes = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
	assert_eq!(&bytes[..], b"Source evidence");
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"hide-file-message", "effect":"deny", "subjects":{"any":true},
		"actions":["message.read"],
		"resources":{"kinds":["message"],"ids":[message["message"]["id"]]}
	}));
	let changed = request(&app, &operator, "POST", "/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy})).await;
	assert_eq!(changed.0, 200);
	assert_eq!(request(&app, &alice, "GET", &download, Value::Null).await.0, 404);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
async fn attachments_cannot_be_rebound_or_linked_from_another_channel() {
	let (f, url, schema) = setup().await;
	let token = f.config.api_token.clone();
	let app = api::router(f.clone());
	let (_, a) = request(&app, &token, "POST", "/api/workspaces", json!({"title":"A","goal":"A"})).await;
	let (_, b) = request(&app, &token, "POST", "/api/workspaces", json!({"title":"B","goal":"B"})).await;
	let a = a["id"].as_str().unwrap();
	let b = b["id"].as_str().unwrap();
	let (status, file) = upload(&app, &token, a, Uuid::new_v4(), b"one").await;
	assert_eq!(status, 200, "{file}");
	let attachment = file["id"].as_str().unwrap();
	let body = |key| json!({"content":"File", "idempotency_key":key,"attachment_ids":[attachment]});
	let other = request(&app, &token, "POST", &format!("/api/workspaces/{b}/thread-messages"), body(Uuid::new_v4())).await;
	assert_eq!(other.0, 404);
	let path = format!("/api/workspaces/{a}/thread-messages");
	assert_eq!(request(&app, &token, "POST", &path, body(Uuid::new_v4())).await.0, 200);
	assert_eq!(request(&app, &token, "POST", &path, body(Uuid::new_v4())).await.0, 409);
	let (status, _) = upload(&app, &token, a, Uuid::new_v4(), b"").await;
	assert_eq!(status, 400);
	cleanup(f, &url, &schema).await;
}
