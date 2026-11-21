//! HTTP regressions for a Thread's flat messages (#1008, ADR 0140): every
//! message belongs directly to its Thread; there are no reply threads.
use crate::channel_fixtures::{ScopedChannel, client_for, decoded, request, scoped_channel};
use crate::endpoint::{EndpointFixture, endpoint};
use reinhardt::test::fixtures::server::test_server_guard;
use serde_json::{Value, json};
use uuid::Uuid;

async fn thread(app: &EndpointFixture, token: &str, title: &str) -> String {
	let (status, value) = request(app, token, "POST", "/api/threads", json!({"title":title})).await;
	assert_eq!(status, 200, "{value}");
	value["id"].as_str().unwrap().to_owned()
}

async fn post(
	app: &EndpointFixture,
	token: &str,
	thread: &str,
	content: &str,
	key: Uuid,
) -> (u16, Value) {
	request(
		app,
		token,
		"POST",
		&format!("/api/threads/{thread}/channel-messages"),
		json!({"content":content,"idempotency_key":key}),
	)
	.await
}

async fn history(app: &EndpointFixture, token: &str, thread: &str, query: &str) -> (u16, Value) {
	request(
		app,
		token,
		"GET",
		&format!("/api/threads/{thread}/message-history?{query}"),
		Value::Null,
	)
	.await
}

#[rstest::rstest]
#[tokio::test]
async fn messages_are_flat_survive_a_new_router_and_do_not_become_tasks(
	#[future] endpoint: EndpointFixture,
) {
	let app = endpoint.await;
	let token = crate::endpoint::tenant_token(&app.runtime.store, "acme").await;
	let thread = thread(&app, &token, "Threads").await;
	let (status, first) = post(&app, &token, &thread, "A question", Uuid::new_v4()).await;
	assert_eq!(status, 200, "{first}");
	let (status, second) = post(
		&app,
		&token,
		&thread,
		"Supporting information",
		Uuid::new_v4(),
	)
	.await;
	assert_eq!(status, 200, "{second}");
	assert!(
		second.get("thread_id").is_none(),
		"a message names no reply thread: {second}"
	);
	let restarted = test_server_guard(
		aidash_server::routes()
			.with_di_context(app.context.clone())
			.into_server(),
	)
	.await;
	let client = client_for(&restarted.url, &token).await;
	let (status, channel) = decoded(
		client
			.get(&format!("/api/threads/{thread}/message-history?limit=20"))
			.await
			.unwrap(),
	);
	assert_eq!(status, 200, "{channel}");
	let messages = channel["messages"].as_array().unwrap();
	assert_eq!(messages.len(), 2, "{channel}");
	assert_eq!(messages[0]["message"]["content"], "A question");
	assert_eq!(messages[1]["message"]["content"], "Supporting information");
	let (status, snapshot) = decoded(client.get(&format!("/api/threads/{thread}")).await.unwrap());
	assert_eq!(status, 200, "{snapshot}");
	assert!(snapshot["tasks"].as_array().unwrap().is_empty());
}

#[rstest::rstest]
#[tokio::test]
async fn duplicate_message_reuses_id_but_changed_content_conflicts(
	#[future] endpoint: EndpointFixture,
) {
	let app = endpoint.await;
	let token = crate::endpoint::tenant_token(&app.runtime.store, "acme").await;
	let thread = thread(&app, &token, "Idempotency").await;
	let key = Uuid::new_v4();
	let (status, first) = post(&app, &token, &thread, "Original", key).await;
	assert_eq!(status, 200, "{first}");
	let (status, second) = post(&app, &token, &thread, "Original", key).await;
	assert_eq!(status, 200, "{second}");
	assert_eq!(first["message"]["id"], second["message"]["id"]);
	assert_eq!(post(&app, &token, &thread, "Changed", key).await.0, 409);
}

#[rstest::rstest]
#[tokio::test]
async fn cursors_cannot_cross_thread_boundaries(#[future] endpoint: EndpointFixture) {
	let app = endpoint.await;
	let token = crate::endpoint::tenant_token(&app.runtime.store, "acme").await;
	let a = thread(&app, &token, "A").await;
	let b = thread(&app, &token, "B").await;
	let (status, message) = post(&app, &token, &a, "Private to A", Uuid::new_v4()).await;
	assert_eq!(status, 200, "{message}");
	let id = message["message"]["id"].as_str().unwrap();
	let query = format!("before={id}");
	assert_eq!(history(&app, &token, &b, &query).await.0, 404);
	let (status, page) = history(&app, &token, &a, &query).await;
	assert_eq!(status, 200, "{page}");
	assert!(page["messages"].as_array().unwrap().is_empty());
}

#[rstest::rstest]
#[tokio::test]
async fn message_history_pages_without_duplicates_and_validates_inputs(
	#[future] endpoint: EndpointFixture,
) {
	let app = endpoint.await;
	let token = crate::endpoint::tenant_token(&app.runtime.store, "acme").await;
	let thread = thread(&app, &token, "Paging").await;
	for content in ["first", "second", "third"] {
		let (status, value) = post(&app, &token, &thread, content, Uuid::new_v4()).await;
		assert_eq!(status, 200, "{value}");
	}
	let (_, latest) = history(&app, &token, &thread, "limit=2").await;
	assert_eq!(latest["messages"].as_array().unwrap().len(), 2);
	assert_eq!(latest["messages"][0]["message"]["content"], "second");
	assert_eq!(latest["messages"][1]["message"]["content"], "third");
	let before = latest["next_before"].as_str().unwrap();
	let query = format!("limit=2&before={before}");
	let (_, older) = history(&app, &token, &thread, &query).await;
	assert_eq!(older["messages"].as_array().unwrap().len(), 1);
	assert_eq!(older["messages"][0]["message"]["content"], "first");
	assert!(older["next_before"].is_null());
	for query in ["limit=0", "limit=101"] {
		assert_eq!(history(&app, &token, &thread, query).await.0, 400);
	}
	let blank = post(&app, &token, &thread, "   ", Uuid::new_v4()).await;
	assert_eq!(blank.0, 400);
	for field in ["sender", "thread_id"] {
		let mut body = json!({"content":"spoof","idempotency_key":Uuid::new_v4()});
		body[field] = json!("00000000-0000-0000-0000-000000000001");
		let spoof = client_for(&app.server.url, &token)
			.await
			.post(
				&format!("/api/threads/{thread}/channel-messages"),
				&body,
				"json",
			)
			.await
			.unwrap();
		// Typed JSON rejection preserves the original status and diagnostic text.
		assert!(spoof.text().contains(&format!("unknown field `{field}`")));
		crate::endpoint::assert_json_rejection(spoof, 422);
	}
}

#[rstest::rstest]
#[tokio::test]
async fn concurrent_identical_submissions_create_one_message(#[future] endpoint: EndpointFixture) {
	let app = endpoint.await;
	let token = crate::endpoint::tenant_token(&app.runtime.store, "acme").await;
	let thread = thread(&app, &token, "Concurrent").await;
	let key = Uuid::new_v4();
	let (a, b) = tokio::join!(
		post(&app, &token, &thread, "one", key),
		post(&app, &token, &thread, "one", key)
	);
	assert_eq!(a.0, 200, "{}", a.1);
	assert_eq!(b.0, 200, "{}", b.1);
	assert_eq!(a.1["message"]["id"], b.1["message"]["id"]);
	let (_, page) = history(&app, &token, &thread, "").await;
	assert_eq!(page["messages"].as_array().unwrap().len(), 1);
}

#[rstest::rstest]
#[tokio::test]
async fn scoped_history_filters_records_and_rechecks_post_authority(
	#[future] scoped_channel: ScopedChannel,
) {
	let ScopedChannel {
		app,
		mut policy,
		token: alice,
		thread,
	} = Box::pin(scoped_channel).await;
	let deployment_administrator = app.runtime.config.api_token.clone();
	let other = crate::endpoint::tenant_token(&app.runtime.store, "other").await;
	let foreign = self::thread(&app, &other, "Another Tenant's").await;
	let mut ids = Vec::new();
	for text in ["first", "hidden", "last"] {
		let (status, value) = post(&app, &alice, &thread, text, Uuid::new_v4()).await;
		assert_eq!(status, 200, "{value}");
		assert_eq!(value["message"]["sender"], "alice");
		ids.push(value["message"]["id"].as_str().unwrap().to_owned());
	}
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"hide-one", "effect":"deny", "subjects":{"any":true},
		"actions":["message.read"], "resources":{"kinds":["message"],"ids":[ids[1]]}
	}));
	let updated = request(
		&app,
		&deployment_administrator,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
	)
	.await;
	assert_eq!(updated.0, 200, "{}", updated.1);
	let (status, page) = history(&app, &alice, &thread, "limit=1").await;
	assert_eq!(status, 200, "{page}");
	assert_eq!(page["messages"][0]["message"]["content"], "last");
	assert_eq!(page["next_before"], ids[2]);
	let query = format!("limit=1&before={}", ids[2]);
	let (status, older) = history(&app, &alice, &thread, &query).await;
	assert_eq!(status, 200, "{older}");
	assert_eq!(older["messages"][0]["message"]["content"], "first");
	assert!(older["next_before"].is_null());
	assert!(!older.to_string().contains("hidden"));
	let query = format!("before={}", ids[1]);
	assert_eq!(history(&app, &alice, &thread, &query).await.0, 404);
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"read-only", "effect":"deny", "subjects":{"any":true},
		"actions":["message.create"], "resources":{"kinds":["thread"]}
	}));
	let updated = request(
		&app,
		&deployment_administrator,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":policy}),
	)
	.await;
	assert_eq!(updated.0, 200, "{}", updated.1);
	let denied = post(&app, &alice, &thread, "denied", Uuid::new_v4()).await;
	assert_eq!(denied.0, 403);
	assert_eq!(history(&app, &alice, &foreign, "").await.0, 404);
}
