mod common;

use aidash::{
	api,
	authorization::{Authorization, policy::PolicyBundle},
	config::OidcConfig,
};
use axum::{Router, body::Body, http::Request};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

async fn call(
	app: &Router,
	method: &str,
	path: &str,
	cookie: bool,
	csrf: bool,
	context: Option<&str>,
	bearer: bool,
	body: Value,
) -> (u16, Value) {
	let mut request = Request::builder()
		.method(method)
		.uri(path)
		.header("content-type", "application/json");
	if cookie {
		request = request.header("cookie", "aidash-session=fixture-session");
	}
	if csrf {
		request = request
			.header("origin", "http://127.0.0.1:8080")
			.header("x-aidash-csrf", "fixture-csrf");
	}
	if let Some(context) = context {
		request = request.header("x-aidash-context", context);
	}
	if bearer {
		request = request.header("authorization", "Bearer operator-execution-fixture");
	}
	let response = app
		.clone()
		.oneshot(request.body(Body::from(body.to_string())).unwrap())
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

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn unmapped_identity_stays_denied_until_operator_approves_existing_user() {
	let (mut federation, url, schema) = common::setup().await;
	federation.config.oidc = Some(OidcConfig {
		issuer: "http://127.0.0.1:18099/realms/test".into(),
		client_id: "aidash".into(),
		client_secret: "test-secret".into(),
		public_origin: "http://127.0.0.1:8080".into(),
		keycloak_admin_url: "http://127.0.0.1:18099/admin/realms/test".into(),
		status_client_id: "status".into(),
		status_client_secret: "status-secret".into(),
		session_absolute_seconds: 43_200,
		session_idle_seconds: 1_800,
	});
	let policy: PolicyBundle = serde_json::from_value(json!({
		"tenant":"acme","subjects":{"alice":{"kind":"user"}},"policies":[]
	}))
	.unwrap();
	Authorization {
		pool: federation.store.pool.clone(),
	}
	.replace("acme", 0, policy, "operator")
	.await
	.unwrap();
	let identity_id = Uuid::new_v4();
	let insert_identity = Query::insert()
		.into_table(Alias::new("dashboard_identities"))
		.columns(["id", "issuer", "subject", "last_valid_at"].map(Alias::new))
		.values_panic([
			Expr::cust("$1"),
			Expr::cust("$2"),
			Expr::cust("$3"),
			Expr::cust("clock_timestamp()"),
		])
		.to_string(PostgresQueryBuilder);
	sqlx::query(&insert_identity)
		.bind(identity_id)
		.bind("http://127.0.0.1:18099/realms/test")
		.bind("keycloak-user-1")
		.execute(&federation.store.pool)
		.await
		.unwrap();
	let insert_session = Query::insert()
		.into_table(Alias::new("dashboard_sessions"))
		.columns(
			[
				"id",
				"token_hash",
				"csrf_hash",
				"identity_id",
				"created_at",
				"last_activity_at",
				"expires_at",
			]
			.map(Alias::new),
		)
		.values_panic([
			Expr::cust("$1"),
			Expr::cust("$2"),
			Expr::cust("$3"),
			Expr::cust("$4"),
			Expr::cust("clock_timestamp()"),
			Expr::cust("clock_timestamp()"),
			Expr::cust("clock_timestamp()+interval '12 hours'"),
		])
		.to_string(PostgresQueryBuilder);
	sqlx::query(&insert_session)
		.bind(Uuid::new_v4())
		.bind(Sha256::digest(b"fixture-session").to_vec())
		.bind(Sha256::digest(b"fixture-csrf").to_vec())
		.bind(identity_id)
		.execute(&federation.store.pool)
		.await
		.unwrap();
	let app = api::router(federation.clone());
	let (status, session) = call(
		&app,
		"GET",
		"/auth/session",
		true,
		false,
		None,
		false,
		Value::Null,
	)
	.await;
	assert_eq!(status, 200);
	assert_eq!(session["mappings"], json!([]));
	assert_eq!(
		call(
			&app,
			"GET",
			"/api/session",
			true,
			false,
			None,
			false,
			Value::Null
		)
		.await
		.0,
		403
	);
	assert_eq!(
		call(
			&app,
			"POST",
			"/auth/registration",
			true,
			false,
			None,
			false,
			Value::Null
		)
		.await
		.0,
		403
	);
	let (status, first) = call(
		&app,
		"POST",
		"/auth/registration",
		true,
		true,
		None,
		false,
		Value::Null,
	)
	.await;
	assert_eq!(status, 200);
	assert_eq!(first["status"], "pending");
	let (_, duplicate) = call(
		&app,
		"POST",
		"/auth/registration",
		true,
		true,
		None,
		false,
		Value::Null,
	)
	.await;
	assert_eq!(duplicate["id"], first["id"]);
	let path = format!(
		"/api/dashboard/registrations/{}/approve",
		first["id"].as_str().unwrap()
	);
	let (status, mapping) = call(
		&app,
		"POST",
		&path,
		false,
		false,
		None,
		true,
		json!({"tenant":"acme","subject":"alice"}),
	)
	.await;
	assert_eq!(status, 200, "{mapping}");
	let context = format!("mapping:{}", mapping["id"].as_str().unwrap());
	let (status, session) = call(
		&app,
		"GET",
		"/api/session",
		true,
		false,
		Some(&context),
		false,
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{session}");
	assert_eq!(session["access"]["subject"], "alice");
	let path = format!(
		"/api/dashboard/mappings/{}/disable",
		mapping["id"].as_str().unwrap()
	);
	assert_eq!(
		call(&app, "POST", &path, false, false, None, true, Value::Null)
			.await
			.0,
		204
	);
	assert_eq!(
		call(
			&app,
			"GET",
			"/api/session",
			true,
			false,
			Some(&context),
			false,
			Value::Null
		)
		.await
		.0,
		403
	);
	assert_eq!(
		call(
			&app,
			"GET",
			"/api/session",
			false,
			false,
			None,
			true,
			Value::Null
		)
		.await
		.0,
		200
	);
	assert_eq!(
		call(
			&app,
			"POST",
			"/auth/logout",
			true,
			true,
			None,
			false,
			Value::Null
		)
		.await
		.0,
		204
	);
	assert_eq!(
		call(
			&app,
			"GET",
			"/auth/session",
			true,
			false,
			None,
			false,
			Value::Null
		)
		.await
		.0,
		401
	);
	common::cleanup(federation, &url, &schema).await;
}
