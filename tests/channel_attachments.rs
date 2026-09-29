mod common;

use aidash::api;
use axum::{Router, body::Body, http::Request};
use common::{TestEnvironment, bootstrap, cleanup, request, setup, test_environment};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

async fn upload(
	app: &Router,
	token: &str,
	workspace: &str,
	key: Uuid,
	bytes: &[u8],
) -> (u16, Value) {
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
	let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
		.await
		.unwrap();
	let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
	(status, value)
}

#[rstest::rstest]
#[tokio::test]
async fn attachment_upload_is_idempotent_and_download_requires_current_message_access(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
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
	assert_eq!(
		upload(&app, &alice, &workspace, key, b"Changed bytes")
			.await
			.0,
		409
	);
	let attachment = file["id"].as_str().unwrap();
	let message_key = Uuid::new_v4();
	let message_path = format!("/api/workspaces/{workspace}/thread-messages");
	policy["subjects"]["bob"] = json!({"kind":"user"});
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"bob-post-only", "effect":"deny", "subjects":{"ids":["bob"]},
		"actions":["workspace.read","message.read"],
		"resources":{"kinds":["workspace","message"]}
	}));
	let (status, changed) = request(
		&app,
		&operator,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy.clone()}),
	)
	.await;
	assert_eq!(status, 200, "{changed}");
	let (status, credential) = request(
		&app,
		&operator,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	assert_eq!(status, 200, "{credential}");
	let bob = credential["token"].as_str().unwrap();
	let (status, foreign) = request(
		&app,
		bob,
		"POST",
		&message_path,
		json!({"content":"Borrowed upload", "idempotency_key":Uuid::new_v4(),
			"attachment_ids":[attachment]}),
	)
	.await;
	assert_eq!(status, 404, "{foreign}");
	let (status, posted) = request(
		&app,
		bob,
		"POST",
		&message_path,
		json!({"content":"Post-only note", "idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(status, 200, "{posted}");
	assert_eq!(
		request(
			&app,
			bob,
			"GET",
			&format!("/api/workspaces/{workspace}/message-history"),
			Value::Null
		)
		.await
		.0,
		404
	);
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
	let (status, thread) = request(
		&app,
		&alice,
		"POST",
		&format!("/api/workspaces/{workspace}/threads"),
		json!({"root_message_id":message["message"]["id"]}),
	)
	.await;
	assert_eq!(status, 200, "{thread}");
	policy["policies"]
		.as_array_mut()
		.unwrap()
		.iter_mut()
		.find(|rule| rule["id"] == "bob-post-only")
		.unwrap()["actions"] = json!(["message.read"]);
	let (status, changed) = request(
		&app,
		&operator,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":policy.clone()}),
	)
	.await;
	assert_eq!(status, 200, "{changed}");
	let (status, posted_without_message_read) = request(
		&app,
		bob,
		"POST",
		&message_path,
		json!({"content":"Read-only workspace post", "idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(status, 200, "{posted_without_message_read}");
	let (status, hidden_reply) = request(
		&app,
		bob,
		"POST",
		&message_path,
		json!({"content":"Invisible root", "thread_id":thread["id"],
			"idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(status, 404, "{hidden_reply}");
	let download = format!("/api/workspaces/{workspace}/attachments/{attachment}");
	let response = app
		.clone()
		.oneshot(
			Request::get(&download)
				.header("authorization", format!("Bearer {alice}"))
				.body(Body::empty())
				.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(response.status(), 200);
	assert_eq!(response.headers()["x-content-type-options"], "nosniff");
	assert!(
		response.headers()["content-disposition"]
			.to_str()
			.unwrap()
			.starts_with("attachment;")
	);
	let bytes = axum::body::to_bytes(response.into_body(), 1024)
		.await
		.unwrap();
	assert_eq!(&bytes[..], b"Source evidence");
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"hide-file-message", "effect":"deny", "subjects":{"any":true},
		"actions":["message.read"],
		"resources":{"kinds":["message"],"ids":[message["message"]["id"]]}
	}));
	let changed = request(
		&app,
		&operator,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":3,"bundle":policy}),
	)
	.await;
	assert_eq!(changed.0, 200);
	assert_eq!(
		request(&app, &alice, "GET", &download, Value::Null).await.0,
		404
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn attachments_cannot_be_rebound_or_linked_from_another_channel(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let token = f.config.api_token.clone();
	let app = api::router(f.clone());
	let (_, a) = request(
		&app,
		&token,
		"POST",
		"/api/workspaces",
		json!({"title":"A","goal":"A"}),
	)
	.await;
	let (_, b) = request(
		&app,
		&token,
		"POST",
		"/api/workspaces",
		json!({"title":"B","goal":"B"}),
	)
	.await;
	let a = a["id"].as_str().unwrap();
	let b = b["id"].as_str().unwrap();
	let (status, file) = upload(&app, &token, a, Uuid::new_v4(), b"one").await;
	assert_eq!(status, 200, "{file}");
	let attachment = file["id"].as_str().unwrap();
	let body = |key| json!({"content":"File", "idempotency_key":key,"attachment_ids":[attachment]});
	let other = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{b}/thread-messages"),
		body(Uuid::new_v4()),
	)
	.await;
	assert_eq!(other.0, 404);
	let path = format!("/api/workspaces/{a}/thread-messages");
	assert_eq!(
		request(&app, &token, "POST", &path, body(Uuid::new_v4()))
			.await
			.0,
		200
	);
	assert_eq!(
		request(&app, &token, "POST", &path, body(Uuid::new_v4()))
			.await
			.0,
		409
	);
	let (status, _) = upload(&app, &token, a, Uuid::new_v4(), b"").await;
	assert_eq!(status, 400);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn media_only_channel_message_preserves_attachment_order_and_retry_identity(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&_test_environment).await;
	let token = f.config.api_token.clone();
	let app = api::router(f.clone());
	let (_, workspace) = request(
		&app,
		&token,
		"POST",
		"/api/workspaces",
		json!({"title":"Media","goal":"Inspect media"}),
	)
	.await;
	let workspace = workspace["id"].as_str().unwrap();
	let (_, first) = upload(&app, &token, workspace, Uuid::new_v4(), b"first").await;
	let (_, second) = upload(&app, &token, workspace, Uuid::new_v4(), b"second").await;
	let mut ids = [
		second["id"].as_str().unwrap(),
		first["id"].as_str().unwrap(),
	];
	ids.sort_unstable();
	ids.reverse();
	let path = format!("/api/workspaces/{workspace}/thread-messages");
	let key = Uuid::new_v4();
	let body = json!({"content":"","idempotency_key":key,"attachment_ids":ids});
	let (status, posted) = request(&app, &token, "POST", &path, body.clone()).await;
	assert_eq!(status, 200, "{posted}");
	assert_eq!(posted["attachments"][0]["id"], ids[0]);
	assert_eq!(posted["attachments"][1]["id"], ids[1]);
	let (status, replay) = request(&app, &token, "POST", &path, body).await;
	assert_eq!(status, 200, "{replay}");
	assert_eq!(replay["message"]["id"], posted["message"]["id"]);
	let (status, _) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"content":"","idempotency_key":key,"attachment_ids":[ids[1],ids[0]]}),
	)
	.await;
	assert_eq!(status, 409);
	let (status, history) = request(
		&app,
		&token,
		"GET",
		&format!("/api/workspaces/{workspace}/message-history"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{history}");
	let saved = history["messages"]
		.as_array()
		.unwrap()
		.iter()
		.find(|item| item["message"]["id"] == posted["message"]["id"])
		.unwrap();
	assert_eq!(saved["attachments"][0]["id"], ids[0]);
	assert_eq!(saved["attachments"][1]["id"], ids[1]);
	// A pre-upgrade message has a sorted digest and all migrated positions at
	// zero. Its original unsorted submission must remain replayable.
	let message_id: Uuid = posted["message"]["id"].as_str().unwrap().parse().unwrap();
	let mut canonical = ids;
	canonical.sort_unstable();
	let legacy_digest = format!("{:x}", Sha256::digest(canonical.join("\n").as_bytes()));
	sqlx::query(
		&Query::update()
			.table(Alias::new("channel_message_context"))
			.value(Alias::new("attachment_digest"), Expr::cust("$2"))
			.and_where(Expr::col(Alias::new("message_id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(message_id)
	.bind(legacy_digest)
	.execute(&f.store.pool)
	.await
	.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("channel_attachments"))
			.value(Alias::new("position"), Expr::value(0))
			.and_where(Expr::col(Alias::new("message_id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(message_id)
	.execute(&f.store.pool)
	.await
	.unwrap();
	let (status, replay) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"content":"","idempotency_key":key,"attachment_ids":ids}),
	)
	.await;
	assert_eq!(status, 200, "{replay}");
	assert_eq!(replay["message"]["id"], posted["message"]["id"]);
	cleanup(f, &url, &schema).await;
}
