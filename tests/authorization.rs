use aidash::{
	api,
	authorization::{
		Authorization,
		policy::{Evaluation, PolicyBundle},
	},
	config::Config,
	federation::Federation,
	registry::Registry,
	store::Store,
};
use axum::{Router, body::Body, http::Request};
use serde_json::{Value, json};
use sqlx::{
	Connection, Executor,
	postgres::{PgConnection, PgPoolOptions},
};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

async fn setup() -> (Router, Store, String, String) {
	let url = std::env::var("AIDASH_TEST_DATABASE_URL")
		.expect("AIDASH_TEST_DATABASE_URL must name a disposable PostgreSQL database");
	let schema = format!("authorization_{}", Uuid::new_v4().simple());
	// SeaQuery has no CREATE/DROP SCHEMA builder; these DDL statements isolate fixtures.
	let mut admin = PgConnection::connect(&url).await.unwrap();
	admin
		.execute(format!("CREATE SCHEMA {schema}").as_str())
		.await
		.unwrap();
	let search_path = schema.clone();
	let pool = PgPoolOptions::new()
		.max_connections(8)
		.after_connect(move |connection, _| {
			let schema = search_path.clone();
			Box::pin(async move {
				sqlx::query(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::Expr::cust(
							"set_config('search_path', $1, false)",
						))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(&schema)
				.execute(&mut *connection)
				.await?;
				sqlx::query(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::Expr::cust(
							"SET_CONFIG('application_name', $1, FALSE)",
						))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(&schema)
				.execute(connection)
				.await?;
				Ok(())
			})
		})
		.connect(&url)
		.await
		.unwrap();
	aidash::store::Store::migrate(&pool).await.unwrap();
	let store = Store::from_pool(pool.clone(), "aidash://authorization-test".into())
		.await
		.unwrap();
	let federation = Federation {
		store: store.clone(),
		registry: Registry::new(pool, &store.node_id),
		config: Config {
			node_id: store.node_id.clone(),
			endpoint: "http://localhost:8080".into(),
			listen: "127.0.0.1:0".parse().unwrap(),
			database_url: url.clone(),
			nats_url: "nats://127.0.0.1:4222".into(),
			api_token: "authorization-test-token".into(),
			web_dir: "web/dist".into(),
			lease_seconds: 30,
		},
		client: reqwest::Client::new(),
		notify: Arc::new(tokio::sync::Notify::new()),
	};
	(api::router(federation), store, url, schema)
}

async fn request(app: &Router, path: &str, body: Value, authenticated: bool) -> (u16, Value) {
	let mut request = Request::builder()
		.method("POST")
		.uri(path)
		.header("content-type", "application/json");
	if authenticated {
		request = request.header("authorization", "Bearer authorization-test-token");
	}
	let response = app
		.clone()
		.oneshot(request.body(Body::from(body.to_string())).unwrap())
		.await
		.unwrap();
	let status = response.status().as_u16();
	let body = axum::body::to_bytes(response.into_body(), 1_048_576)
		.await
		.unwrap();
	(status, serde_json::from_slice(&body).unwrap_or(Value::Null))
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

async fn get(app: &Router, path: &str) -> (u16, Value) {
	let response = app
		.clone()
		.oneshot(
			Request::get(path)
				.header("authorization", "Bearer authorization-test-token")
				.body(Body::empty())
				.unwrap(),
		)
		.await
		.unwrap();
	let status = response.status().as_u16();
	let body = axum::body::to_bytes(response.into_body(), 1_048_576)
		.await
		.unwrap();
	(status, serde_json::from_slice(&body).unwrap())
}

async fn cleanup(store: Store, url: &str, schema: &str) {
	store.control_pool.close().await;
	store.pool.close().await;
	// SeaQuery has no CREATE/DROP SCHEMA builder; these DDL statements isolate fixtures.
	let mut admin = PgConnection::connect(url).await.unwrap();
	admin
		.execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
		.await
		.unwrap();
}

async fn scoped_request(
	app: &Router,
	token: &str,
	method: &str,
	path: &str,
	body: Value,
) -> (u16, Value) {
	let response = app
		.clone()
		.oneshot(
			Request::builder()
				.method(method)
				.uri(path)
				.header("authorization", format!("Bearer {token}"))
				.header("content-type", "application/json")
				.body(Body::from(body.to_string()))
				.unwrap(),
		)
		.await
		.unwrap();
	let status = response.status().as_u16();
	let bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
		.await
		.unwrap();
	(
		status,
		serde_json::from_slice(&bytes).unwrap_or(Value::Null),
	)
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

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn subject_credentials_enforce_workspace_isolation_and_live_revocation() {
	let (app, store, url, schema) = setup().await;
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
		422
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
		Err(aidash::Error::Forbidden)
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
	let count: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
			.from(sea_orm::sea_query::Alias::new("authorization_credentials"))
			.and_where(sea_orm::sea_query::Expr::cust("token_hash = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(token.as_bytes())
	.fetch_one(&store.pool)
	.await
	.unwrap();
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
	cleanup(store, &url, &schema).await;
}

async fn subject_fixture() -> (Router, Store, String, String, Value, Value) {
	let (app, store, url, schema) = setup().await;
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
	(app, store, url, schema, credential, workspace)
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn subject_streams_recheck_buffered_frames_after_policy_and_credential_revocation() {
	use futures_util::StreamExt;
	use std::time::Duration;
	let (app, store, url, schema, credential, workspace) = subject_fixture().await;
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
		let response = app
			.clone()
			.oneshot(
				Request::get(&stream_path)
					.header("authorization", format!("Bearer {token}"))
					.body(Body::empty())
					.unwrap(),
			)
			.await
			.unwrap();
		assert_eq!(response.status(), 200);
		assert_eq!(response.headers()["cache-control"], "no-store");
		let mut stream = response.into_body().into_data_stream();
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
			let response = app
				.clone()
				.oneshot(
					Request::get(&stream_path)
						.header("authorization", format!("Bearer {token}"))
						.body(Body::empty())
						.unwrap(),
				)
				.await
				.unwrap();
			assert_eq!(
				response.status(),
				403,
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
	cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn credential_revocation_serializes_with_workspace_mutation() {
	use std::time::Duration;
	let (app, store, url, schema, credential, workspace) = subject_fixture().await;
	let token = credential["token"].as_str().unwrap().to_owned();
	let credential_id = Uuid::parse_str(credential["credential"]["id"].as_str().unwrap()).unwrap();
	let path = format!("/api/workspaces/{}", workspace["id"].as_str().unwrap());
	// Pause at the durable event boundary, after the authorized UPDATE and
	// before commit, to test the actual HTTP mutation rather than a dry run.
	let mut barrier = store.pool.begin().await.unwrap();
	sqlx::query(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust(
				"PG_ADVISORY_XACT_LOCK(71003201)",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.execute(&mut *barrier)
	.await
	.unwrap();
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
	tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::Expr::cust("EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name = $1 AND wait_event = 'advisory')")).to_string(sea_orm::sea_query::PostgresQueryBuilder))
                .bind(&schema).fetch_one(&store.pool).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("mutation did not reach its event boundary");
	for statement in [
		sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("authorization_credentials"))
			.value(
				sea_orm::sea_query::Alias::new("revoked_at"),
				sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("authorization_bundles"))
			.value(
				sea_orm::sea_query::Alias::new("updated_at"),
				sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()"),
			)
			.and_where(sea_orm::sea_query::Expr::cust(
				"tenant = 'acme' AND CAST($1 AS UUID) IS NOT NULL",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	] {
		let mut attempt = store.pool.begin().await.unwrap();
		sqlx::query(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"set_config('lock_timeout','100ms',true)",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.execute(&mut *attempt)
		.await
		.unwrap();
		let error = sqlx::query(&statement)
			.bind(credential_id)
			.execute(&mut *attempt)
			.await
			.unwrap_err();
		assert_eq!(
			error.as_database_error().unwrap().code().as_deref(),
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
	cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn credential_issuance_validates_subjects_lifetime_and_tenant_revocation() {
	let (app, store, url, schema, credential, _) = subject_fixture().await;
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
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("authorization_credentials"))
			.value(
				sea_orm::sea_query::Alias::new("created_at"),
				sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP() - INTERVAL '2 HOURS'"),
			)
			.value(
				sea_orm::sea_query::Alias::new("expires_at"),
				sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP() - INTERVAL '1 HOUR'"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(Uuid::parse_str(id).unwrap())
	.execute(&store.pool)
	.await
	.unwrap();
	assert_eq!(
		scoped_request(&app, token, "GET", "/api/state", Value::Null)
			.await
			.0,
		401
	);
	cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn denied_reads_cannot_be_bypassed_through_workspace_update_responses() {
	let (app, store, url, schema, credential, workspace) = subject_fixture().await;
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
	let rejected: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
			.from(sea_orm::sea_query::Alias::new("authorization_decisions"))
			.and_where(sea_orm::sea_query::Expr::cust(
				"action = 'workspace.read' AND decision ->> 'reason' = 'explicit_deny'",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&store.pool)
	.await
	.unwrap();
	assert_eq!(
		rejected, 1,
		"denied request must remain in the audit without committing a mutation"
	);
	cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; see scripts/check.sh"]
async fn authorization_api_enforces_policy_revision_revocation_and_audit() {
	let (app, store, url, schema) = setup().await;
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
	let count: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
			.from(sea_orm::sea_query::Alias::new("authorization_decisions"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&store.pool)
	.await
	.unwrap();
	assert_eq!(
		request(&app, "/api/authorization/acme/simulate", evaluation(), true)
			.await
			.1["allowed"],
		true
	);
	let after: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
			.from(sea_orm::sea_query::Alias::new("authorization_decisions"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&store.pool)
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
	let history: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
			.from(sea_orm::sea_query::Alias::new("authorization_revisions"))
			.and_where(sea_orm::sea_query::Expr::cust("tenant = 'acme'"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&store.pool)
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
	cleanup(store, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; see scripts/check.sh"]
async fn authorization_transaction_blocks_revocation_and_concurrent_updates_keep_history_consistent()
 {
	let (_app, store, url, schema) = setup().await;
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
	let mut concurrent = store.pool.begin().await.unwrap();
	sqlx::query(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust(
				"set_config('lock_timeout','100ms',true)",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.execute(&mut *concurrent)
	.await
	.unwrap();
	let error = sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("authorization_bundles"))
			.value(
				sea_orm::sea_query::Alias::new("revision"),
				sea_orm::sea_query::Expr::cust("revision + 1"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("tenant = 'acme'"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.execute(&mut *concurrent)
	.await
	.unwrap_err();
	assert_eq!(
		error.as_database_error().and_then(|e| e.code()).as_deref(),
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
	assert!(matches!(error, aidash::Error::Conflict(_)));
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
	cleanup(store, &url, &schema).await;
}
