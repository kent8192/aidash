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

/// Local blob store file holding an attachment object's content-addressed bytes.
async fn object_blob(app: &EndpointFixture, object_id: Uuid) -> std::path::PathBuf {
	let (tenant, digest): (String, String) =
		sqlx::query_as("SELECT tenant, digest FROM core_objects WHERE id = $1")
			.bind(object_id)
			.fetch_one(app.runtime.store.pool.fixture_driver())
			.await
			.unwrap();
	app.runtime
		.store
		.capabilities
		.0
		.storage
		.join("tenants")
		.join(aidash_server::capabilities::objects::digest(
			tenant.as_bytes(),
		))
		.join("sha256")
		.join(&digest[..2])
		.join(&digest[2..4])
		.join(&digest)
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
	let deployment_administrator = app.runtime.config.api_token.clone();
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
		&deployment_administrator,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy.clone()}),
	)
	.await;
	assert_eq!(status, 200, "{changed}");
	// A Deployment Administrator holds no Tenant authority: the shared bearer is
	// refused on Tenant credentials, and Bob's first credential comes from the host (#484).
	let (refused, body) = request(
		&app,
		&deployment_administrator,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	assert_eq!(refused, 403, "{body}");
	let bob = &crate::endpoint::subject_credential(&app.runtime.store, "acme", "bob").await;
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
		&deployment_administrator,
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
	// New uploads persist an immutable object and never retain inline bytes.
	let attachment_id: Uuid = attachment.parse().unwrap();
	let (object_id, inline): (Uuid, Option<Vec<u8>>) =
		sqlx::query_as("SELECT object_id, content FROM channel_attachments WHERE id = $1")
			.bind(attachment_id)
			.fetch_one(app.runtime.store.pool.fixture_driver())
			.await
			.unwrap();
	assert!(inline.is_none());
	let object_path = object_blob(&app, object_id).await;
	assert_eq!(
		tokio::fs::read(&object_path).await.unwrap(),
		b"Source evidence"
	);
	// Blobs are verified when written and not re-hashed on every read (#697),
	// so a read refuses bytes whose size no longer matches the object.
	tokio::fs::write(&object_path, b"Tampered").await.unwrap();
	assert_ne!(
		alice_client.get(&download).await.unwrap().status_code(),
		200
	);
	tokio::fs::write(&object_path, b"Source evidence")
		.await
		.unwrap();

	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"hide-file-message", "effect":"deny", "subjects":{"any":true},
		"actions":["message.read"],
		"resources":{"kinds":["message"],"ids":[message["message"]["id"]]}
	}));
	let changed = request(
		&app,
		&deployment_administrator,
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
	let token = crate::endpoint::tenant_token(&app.runtime.store, "acme").await;
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
	let token = crate::endpoint::tenant_token(&f.store, "acme").await;

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
		.execute(f.store.pool.fixture_driver())
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
		.execute(f.store.pool.fixture_driver())
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

#[rstest::rstest]
#[tokio::test]
async fn attachment_upload_accepts_large_files_and_rejects_over_limit(
	#[future] scoped_channel: ScopedChannel,
) {
	let ScopedChannel {
		app,
		token,
		workspace,
		..
	} = Box::pin(scoped_channel).await;
	let (status, body) = upload(
		&app,
		&token,
		&workspace,
		Uuid::new_v4(),
		&vec![b'x'; 1_200_000],
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let path = format!(
		"/api/workspaces/{workspace}/attachments?filename=evidence.txt&media_type=text%2Fplain&idempotency_key={}",
		Uuid::new_v4()
	);
	let client = client_for(&app.server.url, &token).await;
	let response = client
		.post_raw(
			&path,
			&vec![b'x'; 8 * 1024 * 1024 + 1],
			"application/octet-stream",
		)
		.await
		.unwrap();
	assert_eq!(response.status_code(), 413);
}

#[rstest::rstest]
#[tokio::test]
async fn startup_backfills_legacy_bytes_and_replay_keeps_the_same_object(
	#[future] scoped_channel: ScopedChannel,
) {
	let ScopedChannel {
		app,
		token,
		workspace,
		..
	} = Box::pin(scoped_channel).await;
	let (status, uploaded) =
		upload(&app, &token, &workspace, Uuid::new_v4(), b"legacy bytes").await;
	assert_eq!(status, 200, "{uploaded}");
	let id: Uuid = uploaded["id"].as_str().unwrap().parse().unwrap();
	// Restore the row to its pre-upgrade representation before restarting.
	sqlx::query("UPDATE channel_attachments SET content = $1, object_id = NULL WHERE id = $2")
		.bind(b"legacy bytes".as_slice())
		.bind(id)
		.execute(app.runtime.store.pool.fixture_driver())
		.await
		.unwrap();
	let mut settings = crate::endpoint::settings_for(&app.database.url);
	settings.deployment.id = app.runtime.config.deployment.to_string();
	let start = || async {
		let context =
			reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
		aidash_server::bootstrap::initialize(&context, &settings, app.database.connection.clone())
			.await
			.unwrap();
	};
	start().await;
	let (first, inline): (Uuid, Option<Vec<u8>>) =
		sqlx::query_as("SELECT object_id, content FROM channel_attachments WHERE id = $1")
			.bind(id)
			.fetch_one(app.runtime.store.pool.fixture_driver())
			.await
			.unwrap();
	assert!(inline.is_none());
	let path = object_blob(&app, first).await;
	assert_eq!(tokio::fs::read(path).await.unwrap(), b"legacy bytes");
	start().await;
	let (second,): (Uuid,) =
		sqlx::query_as("SELECT object_id FROM channel_attachments WHERE id = $1")
			.bind(id)
			.fetch_one(app.runtime.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(first, second);

	// A corrupt legacy digest fails startup without deleting its original bytes.
	let (status, corrupt) = upload(&app, &token, &workspace, Uuid::new_v4(), b"legacy bad").await;
	assert_eq!(status, 200, "{corrupt}");
	let corrupt_id: Uuid = corrupt["id"].as_str().unwrap().parse().unwrap();
	sqlx::query(
		"UPDATE channel_attachments SET content = $1, object_id = NULL, sha256 = $2 WHERE id = $3",
	)
	.bind(b"legacy bad".as_slice())
	.bind("0".repeat(64))
	.bind(corrupt_id)
	.execute(app.runtime.store.pool.fixture_driver())
	.await
	.unwrap();
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	assert!(
		aidash_server::bootstrap::initialize(&context, &settings, app.database.connection.clone())
			.await
			.is_err()
	);
	let (object, original): (Option<Uuid>, Vec<u8>) =
		sqlx::query_as("SELECT object_id, content FROM channel_attachments WHERE id = $1")
			.bind(corrupt_id)
			.fetch_one(app.runtime.store.pool.fixture_driver())
			.await
			.unwrap();
	assert!(object.is_none());
	assert_eq!(original, b"legacy bad");
}

#[rstest::rstest]
#[tokio::test]
async fn deleting_a_thread_reclaims_its_attachment_object(#[future] scoped_channel: ScopedChannel) {
	let ScopedChannel {
		app,
		token,
		workspace,
		..
	} = Box::pin(scoped_channel).await;
	let (status, file) = upload(&app, &token, &workspace, Uuid::new_v4(), b"delete me").await;
	assert_eq!(status, 200, "{file}");
	let id: Uuid = file["id"].as_str().unwrap().parse().unwrap();
	let (object_id,): (Uuid,) =
		sqlx::query_as("SELECT object_id FROM channel_attachments WHERE id = $1")
			.bind(id)
			.fetch_one(app.runtime.store.pool.fixture_driver())
			.await
			.unwrap();
	let path = object_blob(&app, object_id).await;
	assert!(path.exists());
	let (status, message) = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{workspace}/thread-messages"),
		json!({"content":"root", "idempotency_key":Uuid::new_v4(), "attachment_ids":[id]}),
	)
	.await;
	assert_eq!(status, 200, "{message}");
	let (status, thread) = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{workspace}/threads"),
		json!({"root_message_id":message["message"]["id"]}),
	)
	.await;
	assert_eq!(status, 200, "{thread}");
	let (status, deleted) = request(
		&app,
		&token,
		"POST",
		&format!(
			"/api/workspaces/{workspace}/threads/{}/delete",
			thread["id"].as_str().unwrap()
		),
		json!({"idempotency_key":Uuid::new_v4(), "files":[]}),
	)
	.await;
	assert_eq!(status, 200, "{deleted}");
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash_server::capabilities::reclamation::run(
		app.runtime.store.clone(),
		rx,
	));
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
	while path.exists() {
		assert!(
			tokio::time::Instant::now() < deadline,
			"attachment object was not reclaimed"
		);
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	let (remaining,): (i64,) = sqlx::query_as("SELECT count(*) FROM core_objects WHERE id = $1")
		.bind(object_id)
		.fetch_one(app.runtime.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(remaining, 0);
}

#[rstest::rstest]
#[tokio::test]
async fn media_upload_validates_decoded_content_and_records_measurements(
	#[future] scoped_channel: ScopedChannel,
) {
	let ScopedChannel {
		app,
		token,
		workspace,
		..
	} = Box::pin(scoped_channel).await;
	let client = client_for(&app.server.url, &token).await;
	async fn send(
		client: &reinhardt::test::APIClient,
		workspace: &str,
		mime: &str,
		bytes: &[u8],
	) -> (u16, Value) {
		let path = format!(
			"/api/workspaces/{workspace}/attachments?filename=sample&media_type={}&idempotency_key={}",
			mime.replace('/', "%2F"),
			Uuid::new_v4()
		);
		decoded(
			client
				.post_raw(&path, bytes, "application/octet-stream")
				.await
				.unwrap(),
		)
	}
	let png = include_bytes!("../../execution/tests/support/media/pixel.png");
	let (status, body) = send(&client, &workspace, "image/png", png).await;
	assert_eq!(status, 200, "{body}");
	let id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
	let dimensions: (Option<i32>, Option<i32>, Option<i64>) =
		sqlx::query_as("SELECT width, height, duration_ms FROM channel_attachments WHERE id = $1")
			.bind(id)
			.fetch_one(app.runtime.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(dimensions, (Some(1), Some(1), None));
	let wav = include_bytes!("../../execution/tests/support/media/one-second.wav");
	let (status, body) = send(&client, &workspace, "audio/wav", wav).await;
	assert_eq!(status, 200, "{body}");
	let id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
	let (duration,): (Option<i64>,) =
		sqlx::query_as("SELECT duration_ms FROM channel_attachments WHERE id = $1")
			.bind(id)
			.fetch_one(app.runtime.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(duration, Some(1000));
	let mut too_long = wav.to_vec();
	too_long.resize(44 + 8_000 * 601, 128);
	let total = too_long.len() as u32;
	too_long[4..8].copy_from_slice(&(total - 8).to_le_bytes());
	too_long[40..44].copy_from_slice(&(total - 44).to_le_bytes());
	let (status, refusal) = send(&client, &workspace, "audio/wav", &too_long).await;
	assert_eq!(status, 400);
	assert_eq!(refusal["error"], "MEDIA_TOO_LARGE");

	let (status, refusal) =
		send(&client, &workspace, "image/png", b"\x89PNG\r\n\x1a\nbroken").await;
	assert_eq!(status, 400);
	assert_eq!(refusal["error"], "MEDIA_UNDECODABLE");
	let oversized = include_bytes!("../../execution/tests/support/media/oversized.png");
	let (status, refusal) = send(&client, &workspace, "image/png", oversized).await;
	assert_eq!(status, 400);
	assert_eq!(refusal["error"], "MEDIA_TOO_LARGE");
	let (status, refusal) = send(&client, &workspace, "audio/wav", b"RIFF\0\0\0\0WAVEbroken").await;
	assert_eq!(status, 400);
	assert_eq!(refusal["error"], "MEDIA_UNDECODABLE");
}

#[rstest::rstest]
#[tokio::test]
async fn attachment_upload_accepts_eight_mib_through_the_http_stack(
	#[future] scoped_channel: ScopedChannel,
) {
	let ScopedChannel {
		app,
		token,
		workspace,
		..
	} = Box::pin(scoped_channel).await;
	let bytes = vec![b'x'; 8 * 1024 * 1024];
	let (status, body) = upload(&app, &token, &workspace, Uuid::new_v4(), &bytes).await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["size_bytes"], 8 * 1024 * 1024);
}

#[rstest::rstest]
#[tokio::test]
async fn startup_grandfathers_legacy_attachments_above_retained_limit(
	#[future] scoped_channel: ScopedChannel,
) {
	let ScopedChannel {
		app,
		token,
		workspace,
		..
	} = Box::pin(scoped_channel).await;
	let (status, uploaded) = upload(&app, &token, &workspace, Uuid::new_v4(), b"original").await;
	assert_eq!(status, 200, "{uploaded}");
	let id: Uuid = uploaded["id"].as_str().unwrap().parse().unwrap();
	let legacy = b"legacy content over quota";
	let digest = aidash_server::capabilities::objects::digest(legacy);
	sqlx::query(
		"UPDATE channel_attachments SET content=$1, object_id=NULL, sha256=$2, size_bytes=$3 WHERE id=$4",
	)
	.bind(legacy.as_slice())
	.bind(&digest)
	.bind(legacy.len() as i64)
	.bind(id)
	.execute(app.runtime.store.pool.fixture_driver())
	.await
	.unwrap();
	let limit = app.runtime.store.capabilities.0.retained_bytes as i64;
	sqlx::query("UPDATE core_quotas SET used_bytes=$1 WHERE tenant=(SELECT tenant FROM channel_attachments WHERE id=$2)")
        .bind(limit).bind(id).execute(app.runtime.store.pool.fixture_driver()).await.unwrap();
	let mut settings = crate::endpoint::settings_for(&app.database.url);
	settings.deployment.id = app.runtime.config.deployment.to_string();
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	aidash_server::bootstrap::initialize(&context, &settings, app.database.connection.clone())
		.await
		.expect("existing inline bytes must not prevent startup");
	let (object, inline): (Uuid, Option<Vec<u8>>) =
		sqlx::query_as("SELECT object_id, content FROM channel_attachments WHERE id=$1")
			.bind(id)
			.fetch_one(app.runtime.store.pool.fixture_driver())
			.await
			.unwrap();
	assert!(inline.is_none());
	assert_eq!(
		tokio::fs::read(object_blob(&app, object).await)
			.await
			.unwrap(),
		legacy
	);
	let charged: i64 = sqlx::query_scalar("SELECT used_bytes FROM core_quotas WHERE tenant=(SELECT tenant FROM channel_attachments WHERE id=$1)")
        .bind(id).fetch_one(app.runtime.store.pool.fixture_driver()).await.unwrap();
	assert_eq!(charged, limit + legacy.len() as i64);
}

#[rstest::rstest]
#[tokio::test]
async fn metered_storage_counts_stored_attachments_once_and_inline_bytes_separately(
	#[future] scoped_channel: ScopedChannel,
) {
	let ScopedChannel {
		app,
		token,
		workspace,
		..
	} = Box::pin(scoped_channel).await;
	let workspace_id = Uuid::parse_str(&workspace).unwrap();
	let tenant: String = sqlx::query_scalar("SELECT tenant FROM workspaces WHERE id=$1")
		.bind(workspace_id)
		.fetch_one(app.runtime.store.pool.fixture_driver())
		.await
		.unwrap();
	async fn sampled(app: &crate::endpoint::EndpointFixture, tenant: &str) -> u64 {
		aidash_server::apps::execution::services::storage_usage::samples(
			&app.runtime.store,
			chrono::Utc::now(),
		)
		.await
		.unwrap()
		.into_iter()
		.find(|s| s.tenant == tenant)
		.unwrap()
		.bytes
	}
	let before = sampled(&app, &tenant).await;
	let bytes = b"stored and metered";
	let (status, uploaded) = upload(&app, &token, &workspace, Uuid::new_v4(), bytes).await;
	assert_eq!(status, 200, "{uploaded}");
	assert_eq!(sampled(&app, &tenant).await, before + bytes.len() as u64);
	let id: Uuid = uploaded["id"].as_str().unwrap().parse().unwrap();
	let inline = b"inline";
	sqlx::query("INSERT INTO channel_attachments (id,tenant,workspace_id,uploaded_by,idempotency_key,filename,media_type,sha256,size_bytes,content) SELECT $1,tenant,workspace_id,uploaded_by,$2,'inline','text/plain',$3,$4,$5 FROM channel_attachments WHERE id=$6")
        .bind(Uuid::new_v4()).bind(Uuid::new_v4())
        .bind(aidash_server::capabilities::objects::digest(inline)).bind(inline.len() as i64)
        .bind(inline.as_slice()).bind(id).execute(app.runtime.store.pool.fixture_driver()).await.unwrap();
	assert_eq!(
		sampled(&app, &tenant).await,
		before + bytes.len() as u64 + inline.len() as u64
	);
}

/// The index entry of a posted image, as (state, last_error, revision, deleted).
async fn image_entry(
	app: &EndpointFixture,
	attachment: &str,
) -> Option<(String, Option<String>, i64, bool)> {
	sqlx::query_as(
		"SELECT state, last_error, revision, deleted FROM semantic_entries WHERE key = $1",
	)
	.bind(format!("attachment-image:{attachment}"))
	.fetch_optional(app.runtime.store.pool.fixture_driver())
	.await
	.unwrap()
}

async fn upload_image(
	app: &EndpointFixture,
	token: &str,
	workspace: &str,
	description: Option<&str>,
) -> (u16, Value) {
	let mut path = format!(
		"/api/workspaces/{workspace}/attachments?filename=photo.png&media_type=image%2Fpng&idempotency_key={}",
		Uuid::new_v4()
	);
	if let Some(text) = description {
		path.push_str(&format!("&description={}", text.replace(' ', "%20")));
	}
	let png = include_bytes!("../../execution/tests/support/media/pixel.png");
	let client = client_for(&app.server.url, token).await;
	decoded(
		client
			.post_raw(&path, png, "application/octet-stream")
			.await
			.unwrap(),
	)
}

#[rstest::rstest]
#[tokio::test]
async fn posted_images_enter_the_index_with_the_description_only_their_writer_edits(
	#[future] scoped_channel: ScopedChannel,
) {
	// Arrange: an image posted before the Workspace had an index, and a second
	// person who may read and post but did not post the image.
	let ScopedChannel {
		app,
		mut policy,
		token: alice,
		workspace,
	} = Box::pin(scoped_channel).await;
	let message_path = format!("/api/workspaces/{workspace}/thread-messages");
	let (status, early) = upload_image(&app, &alice, &workspace, Some("A red bicycle")).await;
	assert_eq!(status, 200, "{early}");
	assert_eq!(early["description"], "A red bicycle");
	let (status, first) = request(&app, &alice, "POST", &message_path,
		json!({"content":"Before the index", "idempotency_key":Uuid::new_v4(), "attachment_ids":[early["id"]]})).await;
	assert_eq!(status, 200, "{first}");
	let early_id = early["id"].as_str().unwrap();
	assert!(image_entry(&app, early_id).await.is_none());
	policy["subjects"]["bob"] = json!({"kind":"user"});
	let (status, changed) = request(
		&app,
		&app.runtime.config.api_token.clone(),
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{changed}");
	let bob = crate::endpoint::subject_credential(&app.runtime.store, "acme", "bob").await;

	// Act: configure a text-only index, post a second image, and index both.
	let spec = json!({"embedding":{"provider":"openai","endpoint":"http://localhost:9","model":"fixture","model_version":"1","dimensions":8},
		"vector":{"provider":"postgres","endpoint":"local","credential_env":null},"enabled":true,"auto_context":false,
		"max_sources":10,"max_results":5,"max_result_tokens":128,"max_input_bytes":256});
	let (status, index) = request(
		&app,
		&alice,
		"POST",
		&format!("/api/workspaces/{workspace}/semantic/index"),
		json!({"expected_revision":0,"spec":spec}),
	)
	.await;
	assert_eq!(status, 200, "{index}");
	let backfilled = image_entry(&app, early_id).await;
	let (status, late) = upload_image(&app, &alice, &workspace, None).await;
	assert_eq!(status, 200, "{late}");
	let (status, second) = request(&app, &alice, "POST", &message_path,
		json!({"content":"After the index", "idempotency_key":Uuid::new_v4(), "attachment_ids":[late["id"]]})).await;
	assert_eq!(status, 200, "{second}");
	let late_id = late["id"].as_str().unwrap();
	let posted = image_entry(&app, late_id).await;
	aidash_server::semantic::worker::sweep(&app.runtime.store)
		.await
		.unwrap();
	let (_, history) = request(
		&app,
		&alice,
		"GET",
		&format!("/api/workspaces/{workspace}/message-history"),
		Value::Null,
	)
	.await;
	let description_path = format!("/api/workspaces/{workspace}/attachments/{late_id}/description");
	let (foreign_status, _) = request(
		&app,
		&bob,
		"PUT",
		&description_path,
		json!({"description":"Not mine"}),
	)
	.await;
	let (own_status, described) = request(
		&app,
		&alice,
		"PUT",
		&description_path,
		json!({"description":"A blue door"}),
	)
	.await;
	let redescribed = image_entry(&app, late_id).await;

	// Assert
	assert_eq!(backfilled.map(|entry| entry.0), Some("PENDING".to_owned()));
	assert_eq!(posted.map(|entry| entry.0), Some("PENDING".to_owned()));
	let listed: Vec<&Value> = history["messages"]
		.as_array()
		.unwrap()
		.iter()
		.flat_map(|message| message["attachments"].as_array().unwrap())
		.collect();
	assert_eq!(listed.len(), 2, "{history}");
	for attachment in &listed {
		assert_eq!(attachment["index_state"], "unsupported", "{history}");
	}
	assert_eq!(
		image_entry(&app, early_id).await.unwrap().1.as_deref(),
		Some("modality")
	);
	assert_eq!(foreign_status, 403);
	assert_eq!(own_status, 200, "{described}");
	assert_eq!(described["description"], "A blue door");
	let redescribed = redescribed.unwrap();
	assert_eq!(
		redescribed.0, "PENDING",
		"an edited description is indexed again"
	);
	assert_eq!(redescribed.2, 2);
}

#[rstest::rstest]
#[tokio::test]
async fn an_image_leaves_the_index_with_its_thread(#[future] scoped_channel: ScopedChannel) {
	// Arrange
	let ScopedChannel {
		app,
		token,
		workspace,
		..
	} = Box::pin(scoped_channel).await;
	let spec = json!({"embedding":{"provider":"openai","endpoint":"http://localhost:9","model":"fixture","model_version":"1","dimensions":8},
		"vector":{"provider":"postgres","endpoint":"local","credential_env":null},"enabled":true,"auto_context":false,
		"max_sources":10,"max_results":5,"max_result_tokens":128,"max_input_bytes":256});
	let (status, index) = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{workspace}/semantic/index"),
		json!({"expected_revision":0,"spec":spec}),
	)
	.await;
	assert_eq!(status, 200, "{index}");
	let (status, image) = upload_image(&app, &token, &workspace, Some("A harbour at dusk")).await;
	assert_eq!(status, 200, "{image}");
	let (status, message) = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{workspace}/thread-messages"),
		json!({"content":"root", "idempotency_key":Uuid::new_v4(), "attachment_ids":[image["id"]]}),
	)
	.await;
	assert_eq!(status, 200, "{message}");
	let (status, thread) = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{workspace}/threads"),
		json!({"root_message_id":message["message"]["id"]}),
	)
	.await;
	assert_eq!(status, 200, "{thread}");
	let id = image["id"].as_str().unwrap();
	assert_eq!(
		image_entry(&app, id).await.map(|entry| entry.3),
		Some(false)
	);

	// Act
	let (status, deleted) = request(
		&app,
		&token,
		"POST",
		&format!(
			"/api/workspaces/{workspace}/threads/{}/delete",
			thread["id"].as_str().unwrap()
		),
		json!({"idempotency_key":Uuid::new_v4(), "files":[]}),
	)
	.await;

	// Assert
	assert_eq!(status, 200, "{deleted}");
	let entry = image_entry(&app, id).await.unwrap();
	assert_eq!((entry.0.as_str(), entry.3), ("DELETED", true));
}

#[rstest::rstest]
#[tokio::test]
async fn only_an_image_takes_a_description(#[future] scoped_channel: ScopedChannel) {
	// Arrange
	let ScopedChannel {
		app,
		token,
		workspace,
		..
	} = Box::pin(scoped_channel).await;
	let path = format!(
		"/api/workspaces/{workspace}/attachments?filename=notes.txt&media_type=text%2Fplain&idempotency_key={}&description=Notes",
		Uuid::new_v4()
	);
	let client = client_for(&app.server.url, &token).await;

	// Act
	let (status, refusal) = decoded(
		client
			.post_raw(&path, b"plain notes", "application/octet-stream")
			.await
			.unwrap(),
	);

	// Assert
	assert_eq!(status, 400, "{refusal}");
}

async fn post_image(app: &EndpointFixture, token: &str, workspace: &str, content: &str) -> String {
	let (status, image) = upload_image(app, token, workspace, None).await;
	assert_eq!(status, 200, "{image}");
	let (status, message) = request(
		app,
		token,
		"POST",
		&format!("/api/workspaces/{workspace}/thread-messages"),
		json!({"content":content, "idempotency_key":Uuid::new_v4(), "attachment_ids":[image["id"]]}),
	)
	.await;
	assert_eq!(status, 200, "{message}");
	image["id"].as_str().unwrap().to_owned()
}

#[rstest::rstest]
#[tokio::test]
async fn posted_images_use_the_image_limit_not_the_text_source_quota(
	#[future] scoped_channel: ScopedChannel,
) {
	// Arrange: room for one text source and one image.
	let ScopedChannel {
		app,
		token,
		workspace,
		..
	} = Box::pin(scoped_channel).await;
	let index_path = format!("/api/workspaces/{workspace}/semantic/index");
	let mut spec = json!({"embedding":{"provider":"openai","endpoint":"http://localhost:9","model":"fixture","model_version":"1","dimensions":8},
		"vector":{"provider":"postgres","endpoint":"local","credential_env":null},"enabled":true,"auto_context":false,
		"max_sources":1,"max_results":5,"max_result_tokens":128,"max_input_bytes":256,"max_images":1});
	let (status, index) = request(
		&app,
		&token,
		"POST",
		&index_path,
		json!({"expected_revision":0,"spec":spec}),
	)
	.await;
	assert_eq!(status, 200, "{index}");
	let first = post_image(&app, &token, &workspace, "First").await;
	let second = post_image(&app, &token, &workspace, "Second").await;

	// Act: add the one text source, then reconfigure the index.
	let (text_status, text) = request(&app, &token, "POST", &format!("/api/workspaces/{workspace}/semantic/entries"),
		json!({"key":"note","source":{"kind":"memory","text":"A note"},"expected_revision":0,"metadata":{}})).await;
	spec["max_results"] = json!(4);
	let (reconfigured, body) = request(
		&app,
		&token,
		"POST",
		&index_path,
		json!({"expected_revision":1,"spec":spec}),
	)
	.await;

	// Assert: images never counted against the text quota, and the image the
	// limit turned away stays turned away after the rebuild.
	assert_eq!(text_status, 200, "{text}");
	assert_eq!(reconfigured, 200, "{body}");
	assert_eq!(image_entry(&app, &first).await.unwrap().0, "PENDING");
	let limited = image_entry(&app, &second).await.unwrap();
	assert_eq!(
		(limited.0.as_str(), limited.1.as_deref()),
		("UNSUPPORTED", Some("limit"))
	);
}
