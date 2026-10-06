//! HTTP regressions using Reinhardt server, API client, DI and database fixtures.
use crate::channel_fixtures::{ScopedChannel, client_for, decoded, request, scoped_channel};
use crate::endpoint::{EndpointFixture, endpoint};
use serde_json::{Value, json};
use uuid::Uuid;

async fn upload(
	app: &EndpointFixture,
	token: &str,
	workspace: &str,
	key: Uuid,
	bytes: &[u8],
) -> (u16, Value) {
	let path = format!(
		"/api/workspaces/{workspace}/attachments?filename=evidence.txt&media_type=text%2Fplain&idempotency_key={key}"
	);
	let client = client_for(&app.server.url, token).await;
	decoded(
		client
			.post_raw(&path, bytes, "application/octet-stream")
			.await
			.unwrap(),
	)
}

#[rstest::rstest]
#[tokio::test]
async fn attachment_upload_is_idempotent_and_download_requires_current_message_access(
	#[future] scoped_channel: ScopedChannel,
) {
	let ScopedChannel {
		app,
		mut policy,
		token: alice,
		workspace,
	} = Box::pin(scoped_channel).await;
	let operator = app.runtime.config.api_token.clone();
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
	let alice_client = client_for(&app.server.url, &alice).await;
	let response = alice_client.get(&download).await.unwrap();
	assert_eq!(response.status_code(), 200, "{}", response.text());
	assert_eq!(response.header("x-content-type-options"), Some("nosniff"));
	assert!(
		response
			.header("content-disposition")
			.unwrap()
			.starts_with("attachment;")
	);
	assert_eq!(response.body().as_ref(), b"Source evidence");
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
}

#[rstest::rstest]
#[tokio::test]
async fn attachments_cannot_be_rebound_or_linked_from_another_channel(
	#[future] endpoint: EndpointFixture,
) {
	let app = endpoint.await;
	let token = app.runtime.config.api_token.clone();
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
}

#[rstest::rstest]
#[tokio::test]
async fn media_only_channel_message_preserves_attachment_order_and_retry_identity(
	#[future] endpoint: EndpointFixture,
) {
	let app = Box::pin(endpoint).await;
	let f = app.runtime.clone();
	let token = f.config.api_token.clone();

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
	{
		let query_bind_1 = message_id;
		let query_bind_2 = legacy_digest;
		sqlx::query(
			&Query::update()
				.table(Alias::new("channel_message_context"))
				.value_expr(
					Alias::new("attachment_digest"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("message_id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
	}
	.unwrap();
	{
		let query_bind_1 = message_id;
		sqlx::query(
			&Query::update()
				.table(Alias::new("channel_attachments"))
				.value_expr(Alias::new("position"), Expr::cust("0"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("message_id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
	}
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
}

use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use sha2::{Digest, Sha256};

use reinhardt::query::SimpleExpr;
