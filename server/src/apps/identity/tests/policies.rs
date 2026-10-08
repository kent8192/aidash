//! Authorization endpoint regressions against the native route and database fixtures.
use crate::endpoint::{EndpointFixture, endpoint};
use aidash_server::apps::identity::models::{
	AuthorizationCredential, AuthorizationDecision, AuthorizationRevision,
};
use aidash_server::{
	authorization::{
		Authorization,
		policy::{Evaluation, PolicyBundle},
	},
	store::Store,
};
use reinhardt::db::orm::Model;
use reinhardt::http::Handler;
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use reinhardt::test::TestResponse;
use reinhardt::{Request, Response};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

fn decoded(response: TestResponse) -> (u16, Value) {
	assert_eq!(
		response.content_type(),
		Some("application/json"),
		"{}",
		response.text()
	);
	(
		response.status_code(),
		response.json_value().expect("JSON endpoint response"),
	)
}

async fn request(
	app: &EndpointFixture,
	path: &str,
	body: Value,
	authenticated: bool,
) -> (u16, Value) {
	let client = if authenticated {
		&app.operator
	} else {
		&app.anonymous
	};
	decoded(client.post(path, &body, "json").await.unwrap())
}

async fn get(app: &EndpointFixture, path: &str) -> (u16, Value) {
	decoded(app.operator.get(path).await.unwrap())
}

async fn scoped_request(
	app: &EndpointFixture,
	token: &str,
	method: &str,
	path: &str,
	body: Value,
) -> (u16, Value) {
	let authorization = format!("Bearer {token}");
	let headers = [("Authorization", authorization.as_str())];
	// reinhardt-web#6672: use per-request credentials only on the declared
	// anonymous client, preserving the baseline isolation between subjects.
	let response = match method {
		"GET" => app
			.anonymous
			.get_with_headers(path, &headers)
			.await
			.unwrap(),
		"POST" => app
			.anonymous
			.post_raw_with_headers(
				path,
				body.to_string().as_bytes(),
				"application/json",
				&headers,
			)
			.await
			.unwrap(),
		"PATCH" => {
			// reinhardt-web#6661: PATCH lacks public per-request headers. Reuse
			// the fixture-owned raw client without creating a credential client.
			let response = app
				.runtime
				.client
				.patch(format!("{}{path}", app.server.url))
				.bearer_auth(token)
				.json(&body)
				.send()
				.await
				.unwrap();
			let status = response.status();
			let headers = response.headers().clone();
			let version = response.version();
			let body = response.bytes().await.unwrap();
			TestResponse::with_body_and_version(status, headers, body, version)
		}
		_ => panic!("unsupported test method: {method}"),
	};
	decoded(response)
}

async fn stream_response(app: &EndpointFixture, path: &str, token: &str) -> Response {
	// APIClient buffers complete responses. Dispatch through the same production
	// fixture-owned production router to pause exactly between server-side SSE frames.
	app.router
		.handle(
			Request::builder()
				.uri(path)
				.header("authorization", format!("Bearer {token}"))
				.build()
				.unwrap(),
		)
		.await
		.unwrap()
}

fn bundle() -> Value {
	json!({
		"tenant":"acme",
		"subjects":{
			"alice":{"kind":"user","roles":["editor"],"groups":["research"],"attributes":{"department":"research"}},
			"agent":{"kind":"agent","roles":["editor"],"attributes":{"department":"research"},"delegated_by":"alice"}
		},
		"groups":{"research":{"roles":["reader"]}},
		"roles":{"reader":{},"editor":{"inherits":["reader"]}},
		"policies":[{
			"id":"department-reader","effect":"allow","subjects":{"roles":["reader"]},
			"actions":["workspace.read"],"resources":{"kinds":["workspace"]},
			"condition":{"op":"eq","left":{"source":"subject","path":"/department"},
				"right":{"source":"resource","path":"/department"}}
		}]
	})
}

fn evaluation() -> Value {
	json!({"subject":"alice","action":"workspace.read","resource":{
        "tenant":"acme","kind":"workspace","id":"research-workspace","attributes":{"department":"research"}},
        "environment":{"network":"internal"}})
}

fn workspace_policy(tenant: &str) -> Value {
	json!({"tenant":tenant,"subjects":{"alice":{"kind":"user"},"bob":{"kind":"user"}},
        "policies":[{"id":"workspace-owner","effect":"allow","subjects":{"ids":["alice"]},
            "actions":["workspace.create","workspace.read","workspace.update","task.create","message.create","workspace.events"],
            "resources":{"kinds":["workspace"]},
            "condition":{"op":"eq","left":{"source":"resource","path":"/owner"},
                "right":{"source":"literal","value":"alice"}}},
            {"id":"workspace-content","effect":"allow","subjects":{"ids":["alice"]},"actions":["task.read","artifact.read","message.read"],"resources":{"kinds":["task","artifact","message"]},"condition":{"op":"eq","left":{"source":"resource","path":"/owner"},"right":{"source":"literal","value":"alice"}}}]})
}

#[rstest::rstest]
#[tokio::test]
async fn subject_credentials_enforce_workspace_isolation_and_live_revocation(
	#[future] endpoint: EndpointFixture,
) {
	let app = Arc::new(endpoint.await);
	let store = app.runtime.store.clone();
	let legacy = store
		.create_workspace("operator-only", "legacy secret")
		.await
		.unwrap();
	let mut tokens = vec![];
	for tenant in ["acme", "other"] {
		assert_eq!(
			request(
				&app,
				&format!("/api/authorization/{tenant}"),
				json!({"expected_revision":0,"bundle":workspace_policy(tenant)}),
				true
			)
			.await
			.0,
			200
		);
		let (status, credential) = request(
			&app,
			&format!("/api/authorization/{tenant}/credentials"),
			json!({"subject":"alice","expires_in_seconds":3600}),
			true,
		)
		.await;
		assert_eq!(status, 200, "credential issuance: {credential}");
		tokens.push(credential);
	}
	let token = tokens[0]["token"].as_str().unwrap();
	let other = tokens[1]["token"].as_str().unwrap();
	let workspace_body = json!({"title":"private","goal":"tenant secret"});
	let (status, workspace) = scoped_request(
		&app,
		token,
		"POST",
		"/api/workspaces",
		workspace_body.clone(),
	)
	.await;
	assert_eq!(status, 200, "workspace creation: {workspace}");
	let id = workspace["id"].as_str().unwrap();
	let path = format!("/api/workspaces/{id}");
	assert_eq!(
		scoped_request(&app, token, "GET", &path, Value::Null)
			.await
			.0,
		200
	);
	assert_eq!(
		scoped_request(&app, other, "GET", &path, Value::Null)
			.await
			.0,
		403
	);
	assert_eq!(
		scoped_request(
			&app,
			token,
			"GET",
			&format!("/api/workspaces/{}", legacy.id),
			Value::Null
		)
		.await
		.0,
		403
	);
	assert_eq!(
		scoped_request(
			&app,
			other,
			"PATCH",
			&path,
			json!({"revision":0,"state":{"stolen":true}})
		)
		.await
		.0,
		403
	);
	assert_eq!(
		scoped_request(
			&app,
			token,
			"PATCH",
			&path,
			json!({"revision":0,"state":{"tenant":"other","owner":"bob"}})
		)
		.await
		.0,
		200
	);
	// Caller-controlled state must not replace the persisted owner used by ABAC.
	assert_eq!(
		scoped_request(&app, token, "GET", &path, Value::Null)
			.await
			.0,
		200
	);
	assert_eq!(
		scoped_request(
			&app,
			token,
			"POST",
			"/api/workspaces",
			json!({"title":"forged","goal":"x","tenant":"other","owner":"alice"})
		)
		.await
		.0,
		422 // Unknown JSON fields preserve the existing request contract.
	);
	let (status, task) = scoped_request(
		&app,
		token,
		"POST",
		&format!("{path}/tasks"),
		json!({"title":"private-task","description":"private-description"}),
	)
	.await;
	assert_eq!(status, 200, "task creation: {task}");
	assert_eq!(task["created_by"], "alice");
	let stored_task = store
		.task(Uuid::parse_str(task["id"].as_str().unwrap()).unwrap())
		.await
		.unwrap();
	assert!(matches!(
		store
			.accept_run(&stored_task, "aidash://untrusted", "agent", "1.0.0")
			.await,
		Err(aidash_server::Error::Forbidden)
	));
	let (status, _) = request(
		&app,
		&format!("/api/tasks/{}/delegate", stored_task.id),
		json!({"node_id":"aidash://untrusted","agent":{"id":"agent","version":"1.0.0"}}),
		true,
	)
	.await;
	assert_eq!(
		status, 403,
		"operator must not bypass missing execution authority"
	);
	assert_eq!(
		scoped_request(
			&app,
			token,
			"POST",
			&format!("{path}/messages"),
			json!({"content":"private-message"})
		)
		.await
		.0,
		200
	);
	let (status, own_state) = scoped_request(&app, token, "GET", "/api/state", Value::Null).await;
	assert_eq!(status, 200);
	assert_eq!(own_state["workspaces"].as_array().unwrap().len(), 1);
	assert_eq!(own_state["tasks"][0]["id"], task["id"]);
	for key in ["registry", "peers", "installations"] {
		assert_eq!(own_state[key], json!([]));
	}
	let (_, other_state) = scoped_request(&app, other, "GET", "/api/state", Value::Null).await;
	for key in [
		"workspaces",
		"tasks",
		"artifacts",
		"events",
		"runs",
		"conversations",
		"human_requests",
	] {
		assert_eq!(other_state[key], json!([]), "leaked {key}");
	}
	assert_eq!(
		scoped_request(&app, other, "GET", "/api/events", Value::Null).await,
		(200, json!([]))
	);
	assert_eq!(
		scoped_request(
			&app,
			other,
			"GET",
			&format!("/api/events?workspace_id={id}"),
			Value::Null
		)
		.await
		.0,
		403
	);
	for path in ["/api/marketplace", "/api/mesh", "/api/authorization/acme"] {
		assert_eq!(
			scoped_request(&app, token, "GET", path, Value::Null)
				.await
				.0,
			403,
			"unscoped path {path}"
		);
	}
	let (_, credentials) = get(&app, "/api/authorization/acme/credentials").await;
	assert!(!credentials.to_string().contains(token));
	assert!(!credentials.to_string().contains("token_hash"));
	assert_eq!(credentials[0]["id"], tokens[0]["credential"]["id"]);
	let mut db = app.database.lease.handle();
	let credentials = AuthorizationCredential::objects()
		.all()
		.all_with_db(&mut db)
		.await
		.unwrap();
	let count = credentials
		.iter()
		.filter(|record| record.token_hash == token.as_bytes())
		.count();
	assert_eq!(count, 0, "plaintext token was stored as a digest");
	let mut changed = workspace_policy("acme");
	changed["policies"] = json!([]);
	assert_eq!(
		request(
			&app,
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":changed}),
			true
		)
		.await
		.0,
		200
	);
	assert_eq!(
		scoped_request(&app, token, "GET", &path, Value::Null)
			.await
			.0,
		403
	);
	assert_eq!(
		scoped_request(&app, token, "POST", "/api/workspaces", workspace_body)
			.await
			.0,
		403
	);
	let (_, hidden) = scoped_request(&app, token, "GET", "/api/state", Value::Null).await;
	assert_eq!(hidden["workspaces"], json!([]));
	let credential_id = tokens[0]["credential"]["id"].as_str().unwrap();
	let revoke = format!("/api/authorization/acme/credentials/{credential_id}/revoke");
	assert_eq!(request(&app, &revoke, json!({}), true).await.0, 200);
	assert_eq!(request(&app, &revoke, json!({}), true).await.0, 200);
	assert_eq!(
		scoped_request(&app, token, "GET", "/api/state", Value::Null)
			.await
			.0,
		401
	);
	assert_eq!(
		scoped_request(
			&app,
			"invalid-subject-token",
			"GET",
			"/api/state",
			Value::Null
		)
		.await
		.0,
		401
	);
}

async fn subject_fixture(endpoint: EndpointFixture) -> (Arc<EndpointFixture>, Store, Value, Value) {
	let app = Arc::new(endpoint);
	let store = app.runtime.store.clone();
	assert_eq!(
		request(
			&app,
			"/api/authorization/acme",
			json!({"expected_revision":0,"bundle":workspace_policy("acme")}),
			true
		)
		.await
		.0,
		200
	);
	let (status, credential) = request(
		&app,
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
		true,
	)
	.await;
	assert_eq!(status, 200);
	let (status, workspace) = scoped_request(
		&app,
		credential["token"].as_str().unwrap(),
		"POST",
		"/api/workspaces",
		json!({"title":"stream","goal":"private"}),
	)
	.await;
	assert_eq!(status, 200);
	(app, store, credential, workspace)
}

#[rstest::rstest]
#[tokio::test]
async fn subject_receives_thread_opened_event_for_a_visible_root_message(
	#[future] endpoint: EndpointFixture,
) {
	use futures_util::StreamExt;
	use std::time::Duration;

	let (app, store, credential, workspace) = subject_fixture(endpoint.await).await;
	let token = credential["token"].as_str().unwrap();
	let workspace_id = Uuid::parse_str(workspace["id"].as_str().unwrap()).unwrap();
	store
		.message(workspace_id, "alice", "Visible root", None)
		.await
		.unwrap();
	let snapshot = store.snapshot(workspace_id).await.unwrap();
	let root_id = snapshot
		.messages
		.iter()
		.find(|message| message.content == "Visible root")
		.unwrap()
		.id
		.to_string();
	let cursor = store
		.events(0, Some(workspace_id), 100)
		.await
		.unwrap()
		.last()
		.unwrap()
		.sequence;
	let stream_path = format!("/api/events/stream?workspace_id={workspace_id}&after={cursor}");
	let mut response = stream_response(&app, &stream_path, token).await;
	assert_eq!(response.status, 200);
	let mut stream = response
		.take_stream_body()
		.expect("native streaming response");
	let (status, thread) = scoped_request(
		&app,
		token,
		"POST",
		&format!("/api/workspaces/{workspace_id}/threads"),
		json!({"root_message_id":root_id}),
	)
	.await;
	assert_eq!(status, 200, "{thread}");
	// This router fixture has no broker supervisor; exercise canonical fallback.
	let frame = tokio::time::timeout(Duration::from_secs(6), stream.next())
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	let frame = String::from_utf8_lossy(&frame);
	assert!(frame.contains("message.thread_opened"), "{frame}");
	assert!(frame.contains(&root_id), "{frame}");
	let (status, events) = scoped_request(
		&app,
		token,
		"GET",
		&format!("/api/events?workspace_id={workspace_id}"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{events}");
	assert!(events.as_array().unwrap().iter().any(|event| {
		event["kind"] == "message.thread_opened"
			&& event["data"]["id"].as_str() == Some(root_id.as_str())
	}));
}

#[rstest::rstest]
#[tokio::test]
async fn subject_streams_recheck_buffered_frames_after_policy_and_credential_revocation(
	#[future] endpoint: EndpointFixture,
) {
	use futures_util::StreamExt;
	use std::time::Duration;
	let (app, store, credential, workspace) = subject_fixture(endpoint.await).await;
	let token = credential["token"].as_str().unwrap();
	let workspace_id = Uuid::parse_str(workspace["id"].as_str().unwrap()).unwrap();
	store
		.message(
			workspace_id,
			"alice",
			"must not escape a revoked stream",
			None,
		)
		.await
		.unwrap();
	let stream_path = format!("/api/events/stream?workspace_id={workspace_id}");
	for revoke_policy in [true, false] {
		let mut response = stream_response(&app, &stream_path, token).await;
		assert_eq!(response.status, 200);
		assert_eq!(response.headers["cache-control"], "no-store");
		let mut stream = response
			.take_stream_body()
			.expect("native streaming response");
		let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
			.await
			.unwrap()
			.unwrap()
			.unwrap();
		assert!(String::from_utf8_lossy(&first).contains("event: mesh"));
		// The first poll buffered both workspace.created and message.created.
		if revoke_policy {
			let mut denied = workspace_policy("acme");
			denied["policies"] = json!([]);
			assert_eq!(
				request(
					&app,
					"/api/authorization/acme",
					json!({"expected_revision":1,"bundle":denied}),
					true
				)
				.await
				.0,
				200
			);
		} else {
			let id = credential["credential"]["id"].as_str().unwrap();
			assert_eq!(
				request(
					&app,
					&format!("/api/authorization/acme/credentials/{id}/revoke"),
					json!({}),
					true
				)
				.await
				.0,
				200
			);
		}
		let next = tokio::time::timeout(Duration::from_secs(2), stream.next())
			.await
			.unwrap()
			.unwrap()
			.unwrap();
		let frame = String::from_utf8_lossy(&next);
		assert!(frame.contains("event: error"), "{frame}");
		assert!(!frame.contains("must not escape"));
		assert!(
			tokio::time::timeout(Duration::from_secs(2), stream.next())
				.await
				.unwrap()
				.is_none()
		);
		if revoke_policy {
			let response = stream_response(&app, &stream_path, token).await;
			assert_eq!(
				response.status, 403,
				"reconnecting must reevaluate authority before HTTP 200"
			);
			assert_eq!(
				request(
					&app,
					"/api/authorization/acme",
					json!({"expected_revision":2,"bundle":workspace_policy("acme")}),
					true
				)
				.await
				.0,
				200
			);
		}
	}
}

#[rstest::rstest]
#[tokio::test]
async fn credential_revocation_serializes_with_workspace_mutation(
	#[future] endpoint: EndpointFixture,
) {
	use std::time::Duration;
	let (app, store, credential, workspace) = subject_fixture(endpoint.await).await;
	let token = credential["token"].as_str().unwrap().to_owned();
	let credential_id = Uuid::parse_str(credential["credential"]["id"].as_str().unwrap()).unwrap();
	let path = format!("/api/workspaces/{}", workspace["id"].as_str().unwrap());
	// Pause at the durable event boundary, after the authorized UPDATE and
	// before commit, to test the actual HTTP mutation rather than a dry run.
	let mut barrier = app.database.connection.begin().await.unwrap();
	let lock = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![Expr::value(71003201_i64).into()],
		))
		.to_string(PostgresQueryBuilder);
	barrier.execute(&lock, vec![]).await.unwrap();
	let patch = tokio::spawn({
		let app = app.clone();
		let token = token.clone();
		let path = path.clone();
		async move {
			scoped_request(
				&app,
				&token,
				"PATCH",
				&path,
				json!({"revision":0,"state":{"committed":true}}),
			)
			.await
		}
	});
	let waiting_query = Query::select()
		.column(Alias::new("pid"))
		.from(Alias::new("pg_stat_activity"))
		.and_where(
			Expr::col(Alias::new("datname")).eq(SimpleExpr::FunctionCall(
				"current_database".into_iden(),
				vec![],
			)),
		)
		.and_where(Expr::col(Alias::new("wait_event")).eq("advisory"))
		.limit(1)
		.to_string(PostgresQueryBuilder);
	tokio::time::timeout(Duration::from_secs(5), async {
		loop {
			if !app
				.database
				.connection
				.fetch_all(&waiting_query, vec![])
				.await
				.unwrap()
				.is_empty()
			{
				break;
			}
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
	})
	.await
	.expect("mutation did not reach its event boundary");
	for statement in [
		Query::update()
			.table(Alias::new("authorization_credentials"))
			.value_expr(Alias::new("revoked_at"), Expr::value(chrono::Utc::now()))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(credential_id)))
			.to_string(PostgresQueryBuilder),
		Query::update()
			.table(Alias::new("authorization_bundles"))
			.value_expr(Alias::new("updated_at"), Expr::value(chrono::Utc::now()))
			.and_where(Expr::col(Alias::new("tenant")).eq("acme"))
			.to_string(PostgresQueryBuilder),
	] {
		let mut attempt = app.database.connection.begin().await.unwrap();
		attempt.execute(&lock_timeout_sql(), vec![]).await.unwrap();
		let error = attempt.execute(&statement, vec![]).await.unwrap_err();
		assert_eq!(
			error.database_error().and_then(|error| error.code()),
			Some("55P03")
		);
		attempt.rollback().await.unwrap();
	}
	barrier.rollback().await.unwrap();
	assert_eq!(patch.await.unwrap().0, 200);
	let revoke = format!("/api/authorization/acme/credentials/{credential_id}/revoke");
	assert_eq!(request(&app, &revoke, json!({}), true).await.0, 200);
	assert_eq!(
		scoped_request(
			&app,
			&token,
			"PATCH",
			&path,
			json!({"revision":1,"state":{}})
		)
		.await
		.0,
		401
	);
	let id = Uuid::parse_str(workspace["id"].as_str().unwrap()).unwrap();
	assert_eq!(
		store.workspace(id).await.unwrap().state,
		json!({"committed":true})
	);
}

#[rstest::rstest]
#[tokio::test]
async fn credential_issuance_validates_subjects_lifetime_and_tenant_revocation(
	#[future] endpoint: EndpointFixture,
) {
	let (app, _, credential, _) = subject_fixture(endpoint.await).await;
	for body in [
		json!({"subject":"missing"}),
		json!({"subject":"alice","expires_in_seconds":0}),
		json!({"subject":"alice","expires_in_seconds":2592001}),
	] {
		assert_eq!(
			request(&app, "/api/authorization/acme/credentials", body, true)
				.await
				.0,
			400
		);
	}
	let token = credential["token"].as_str().unwrap();
	assert_eq!(
		scoped_request(
			&app,
			token,
			"POST",
			"/api/authorization/acme/credentials",
			json!({"subject":"bob"})
		)
		.await
		.0,
		403
	);
	let id = credential["credential"]["id"].as_str().unwrap();
	assert_eq!(
		request(
			&app,
			&format!("/api/authorization/other/credentials/{id}/revoke"),
			json!({}),
			true
		)
		.await
		.0,
		404
	);
	assert_eq!(
		scoped_request(&app, token, "GET", "/api/state", Value::Null)
			.await
			.0,
		200
	);
	let mut disabled = workspace_policy("acme");
	disabled["subjects"]["alice"]["enabled"] = json!(false);
	assert_eq!(
		request(
			&app,
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":disabled}),
			true
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(
			&app,
			"/api/authorization/acme/credentials",
			json!({"subject":"alice"}),
			true
		)
		.await
		.0,
		400
	);
	assert_eq!(
		scoped_request(&app, token, "GET", "/api/state", Value::Null)
			.await
			.0,
		403
	);
	let mut db = app.database.lease.handle();
	let mut expired = AuthorizationCredential::objects()
		.filter(AuthorizationCredential::field_id().eq(Uuid::parse_str(id).unwrap()))
		.get_with_db(&mut db)
		.await
		.unwrap();
	expired.created_at = chrono::Utc::now() - chrono::Duration::hours(2);
	expired.expires_at = chrono::Utc::now() - chrono::Duration::hours(1);
	AuthorizationCredential::objects()
		.update_with_conn(&mut db, &expired)
		.await
		.unwrap();
	assert_eq!(
		scoped_request(&app, token, "GET", "/api/state", Value::Null)
			.await
			.0,
		401
	);
}

#[rstest::rstest]
#[tokio::test]
async fn denied_reads_cannot_be_bypassed_through_workspace_update_responses(
	#[future] endpoint: EndpointFixture,
) {
	let (app, store, credential, workspace) = subject_fixture(endpoint.await).await;
	let mut policy = workspace_policy("acme");
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"read-denied","effect":"deny","subjects":{"any":true},
		"actions":["workspace.read"],"resources":{"kinds":["workspace"]}
	}));
	assert_eq!(
		request(
			&app,
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy}),
			true
		)
		.await
		.0,
		200
	);
	let id = Uuid::parse_str(workspace["id"].as_str().unwrap()).unwrap();
	let token = credential["token"].as_str().unwrap();
	assert_eq!(
		scoped_request(
			&app,
			token,
			"PATCH",
			&format!("/api/workspaces/{id}"),
			json!({"revision":0,"state":{"probe":true}})
		)
		.await
		.0,
		403
	);
	let unchanged = store.workspace(id).await.unwrap();
	assert_eq!(unchanged.revision, 0);
	assert_eq!(unchanged.state, json!({}));
	let mut db = app.database.lease.handle();
	let decisions = AuthorizationDecision::objects()
		.filter(AuthorizationDecision::field_action().eq("workspace.read"))
		.all_with_db(&mut db)
		.await
		.unwrap();
	let rejected = decisions
		.iter()
		.filter(|record| record.decision.0["reason"] == "explicit_deny")
		.count();
	assert_eq!(
		rejected, 1,
		"denied request must remain in the audit without committing a mutation"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn authorization_api_enforces_policy_revision_revocation_and_audit(
	#[future] endpoint: EndpointFixture,
) {
	let app = Arc::new(endpoint.await);
	let mut policy = bundle();
	let update = json!({"expected_revision":0,"bundle":policy});
	assert_eq!(
		request(&app, "/api/authorization/acme", update.clone(), false)
			.await
			.0,
		401
	);
	let (status, created) = request(&app, "/api/authorization/acme", update.clone(), true).await;
	assert_eq!(status, 200, "policy creation: {created}");
	assert_eq!(created["revision"], 1);
	assert_eq!(
		request(&app, "/api/authorization/acme", update, true)
			.await
			.0,
		409
	);
	let path = "/api/authorization/acme/evaluate";
	let (status, allowed) = request(&app, path, evaluation(), true).await;
	assert_eq!(status, 200);
	assert_eq!(allowed["allowed"], true);
	assert_eq!(allowed["revision"], 1);
	assert_eq!(allowed["matched_policies"], json!(["department-reader"]));

	let mut foreign = evaluation();
	foreign["resource"]["tenant"] = json!("other");
	assert_eq!(request(&app, path, foreign, true).await.1["allowed"], false);
	let mut absent = evaluation();
	absent["resource"]["attributes"] = json!({});
	assert_eq!(request(&app, path, absent, true).await.1["allowed"], false);
	let mut db = app.database.lease.handle();
	let count = AuthorizationDecision::objects()
		.count_with_conn(&mut db)
		.await
		.unwrap();
	assert_eq!(
		request(&app, "/api/authorization/acme/simulate", evaluation(), true)
			.await
			.1["allowed"],
		true
	);
	let after = AuthorizationDecision::objects()
		.count_with_conn(&mut db)
		.await
		.unwrap();
	assert_eq!(count, after, "dry-run must not add an audit record");

	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"suspended","effect":"deny","subjects":{"ids":["alice"]},
		"actions":["*"],"resources":{"kinds":["*"]}
	}));
	let (status, _) = request(
		&app,
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
		true,
	)
	.await;
	assert_eq!(status, 200);
	let denied = request(&app, path, evaluation(), true).await.1;
	assert_eq!(denied["allowed"], false);
	assert_eq!(denied["reason"], "explicit_deny");
	assert_eq!(denied["revision"], 2);
	let mut delegated = evaluation();
	delegated["subject"] = json!("agent");
	let denied = request(&app, path, delegated.clone(), true).await.1;
	assert_eq!(
		denied["allowed"], false,
		"agent must not exceed its delegator's authority"
	);
	assert_eq!(denied["reason"], "delegation_denied");

	policy["policies"].as_array_mut().unwrap().pop();
	policy["subjects"]["alice"]["enabled"] = json!(false);
	assert_eq!(
		request(
			&app,
			"/api/authorization/acme",
			json!({"expected_revision":2,"bundle":policy}),
			true
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(&app, path, delegated, true).await.1["allowed"],
		false
	);
	assert_eq!(
		request(&app, path, evaluation(), true).await.1["reason"],
		"subject_disabled"
	);
	let history = AuthorizationRevision::objects()
		.count_with_conn(&mut db)
		.await
		.unwrap();
	assert_eq!(history, 3);
	let (status, snapshot) = get(&app, "/api/authorization/acme").await;
	assert_eq!(status, 200);
	assert_eq!(snapshot["revision"], 3);
	assert_eq!(snapshot["bundle"]["subjects"]["alice"]["enabled"], false);
	let (status, history) = get(&app, "/api/authorization/acme/revisions?after=1&limit=1").await;
	assert_eq!(status, 200);
	assert_eq!(history.as_array().unwrap().len(), 1);
	assert_eq!(history[0]["revision"], 2);
	assert_eq!(history[0]["actor"], "operator");
	let (status, decisions) = get(&app, "/api/authorization/acme/decisions?limit=1").await;
	assert_eq!(status, 200);
	assert_eq!(decisions.as_array().unwrap().len(), 1);
	assert_eq!(decisions[0]["decision"]["allowed"], true);
	assert!(decisions[0].get("attributes").is_none());
	assert_eq!(
		get(&app, "/api/authorization/unknown/decisions").await.1,
		json!([])
	);
}

#[rstest::rstest]
#[tokio::test]
async fn authorization_transaction_blocks_revocation_and_concurrent_updates_keep_history_consistent(
	#[future] endpoint: EndpointFixture,
) {
	let app = endpoint.await;
	let store = app.runtime.store.clone();
	let service = Authorization {
		pool: store.pool.clone(),
	};
	let policy: PolicyBundle = serde_json::from_value(bundle()).unwrap();
	service
		.replace("acme", 0, policy.clone(), "operator")
		.await
		.unwrap();
	let input: Evaluation = serde_json::from_value(evaluation()).unwrap();
	let mut tx = store.pool.begin().await.unwrap();
	let decision = Authorization::evaluate_in_transaction(&mut tx, "acme", &input)
		.await
		.unwrap();
	assert!(decision.allowed);
	let mut concurrent = app.database.connection.begin().await.unwrap();
	concurrent
		.execute(&lock_timeout_sql(), vec![])
		.await
		.unwrap();
	let update = Query::update()
		.table(Alias::new("authorization_bundles"))
		.value_expr(
			Alias::new("revision"),
			Expr::col(Alias::new("revision")).add(1_i64),
		)
		.and_where(Expr::col(Alias::new("tenant")).eq("acme"))
		.to_string(PostgresQueryBuilder);
	let error = concurrent.execute(&update, vec![]).await.unwrap_err();
	assert_eq!(
		error.database_error().and_then(|error| error.code()),
		Some("55P03")
	);
	concurrent.rollback().await.unwrap();
	tx.commit().await.unwrap();
	let mut revoked = policy.clone();
	revoked.subjects.get_mut("alice").unwrap().enabled = false;
	let (one, two) = tokio::join!(
		service.replace("acme", 1, revoked, "operator"),
		service.replace("acme", 1, policy, "operator")
	);
	assert_ne!(one.is_ok(), two.is_ok());
	let error = one.err().or_else(|| two.err()).unwrap();
	assert!(matches!(error, aidash_server::Error::Conflict(_)));
	let snapshot = service.snapshot("acme").await.unwrap();
	let revisions = service.revisions("acme", 0, 100).await.unwrap();
	assert_eq!(snapshot.revision, 2);
	assert_eq!(revisions.len(), 2);
	assert_eq!(
		revisions[1]["document"],
		serde_json::to_value(snapshot.bundle).unwrap()
	);
	assert!(
		service
			.replace(
				"other",
				0,
				serde_json::from_value(bundle()).unwrap(),
				"operator"
			)
			.await
			.is_err()
	);
	assert!(service.revisions("other", 0, 100).await.unwrap().is_empty());
}

fn lock_timeout_sql() -> String {
	Query::select()
		.expr(SimpleExpr::FunctionCall(
			"set_config".into_iden(),
			vec![
				Expr::value("lock_timeout").into(),
				Expr::value("100ms").into(),
				Expr::value(true).into(),
			],
		))
		.to_string(PostgresQueryBuilder)
}
