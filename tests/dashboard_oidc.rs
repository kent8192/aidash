mod common;

use aidash::{
	api,
	authorization::{Authorization, policy::PolicyBundle},
	config::OidcConfig,
};
use axum::{Json, Router, body::Body, http::Request, routing::get};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{
	Arc,
	atomic::{AtomicUsize, Ordering},
};
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn login_prunes_expired_transactions_and_bounds_pending_browser_logins() {
	let (mut federation, url, schema) = common::setup().await;
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let issuer = format!("http://{}/realms/test", listener.local_addr().unwrap());
	let metadata_issuer = issuer.clone();
	let discoveries = Arc::new(AtomicUsize::new(0));
	let discovery_counter = discoveries.clone();
	let fixture = Router::new()
		.route(
			"/realms/test/.well-known/openid-configuration",
			get(move || {
				let issuer = metadata_issuer.clone();
				discovery_counter.fetch_add(1, Ordering::SeqCst);
				async move {
					Json(json!({
						"issuer": issuer,
						"authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
						"token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
						"jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
						"response_types_supported": ["code"],
						"subject_types_supported": ["public"],
						"id_token_signing_alg_values_supported": ["RS256"]
					}))
				}
			}),
		)
		.route(
			"/realms/test/protocol/openid-connect/certs",
			get(|| async { Json(json!({"keys": []})) }),
		);
	let server = tokio::spawn(async move { axum::serve(listener, fixture).await.unwrap() });
	federation.config.oidc = Some(OidcConfig {
		issuer: issuer.clone(),
		client_id: "aidash".into(),
		client_secret: "test-secret".into(),
		public_origin: "http://127.0.0.1:8080".into(),
		keycloak_admin_url: format!("{issuer}/admin"),
		status_client_id: "status".into(),
		status_client_secret: "status-secret".into(),
		session_absolute_seconds: 43_200,
		session_idle_seconds: 1_800,
	});
	let insert = Query::insert()
		.into_table(Alias::new("dashboard_login_transactions"))
		.columns(
			[
				"state_hash",
				"browser_hash",
				"nonce",
				"pkce_verifier",
				"return_to",
				"callback_uri",
				"expires_at",
			]
			.map(Alias::new),
		)
		.values_panic([
			Expr::cust("$1"),
			Expr::cust("$2"),
			Expr::value("nonce"),
			Expr::value("verifier"),
			Expr::value("/"),
			Expr::value("http://127.0.0.1:8080/auth/callback"),
			Expr::cust("clock_timestamp()-interval '1 minute'"),
		])
		.to_string(PostgresQueryBuilder);
	sqlx::query(&insert)
		.bind(Sha256::digest(b"abandoned-state").to_vec())
		.bind(Sha256::digest(b"abandoned-browser").to_vec())
		.execute(&federation.store.pool)
		.await
		.unwrap();
	let app = api::router(federation.clone());
	let mut cookie = None;
	for attempt in 0..9 {
		let mut request = Request::builder().uri("/auth/login");
		if let Some(value) = &cookie {
			request = request.header("cookie", value);
		}
		let response = app
			.clone()
			.oneshot(request.body(Body::empty()).unwrap())
			.await
			.unwrap();
		if attempt < 8 {
			if response.status() != 307 {
				let status = response.status();
				let body = axum::body::to_bytes(response.into_body(), 1_048_576)
					.await
					.unwrap();
				panic!(
					"login returned {status}: {}",
					String::from_utf8_lossy(&body)
				);
			}
			cookie = Some(
				response.headers()["set-cookie"]
					.to_str()
					.unwrap()
					.split(';')
					.next()
					.unwrap()
					.to_owned(),
			);
		} else {
			assert_eq!(response.status(), 409);
		}
	}
	let count_query = Query::select()
		.expr(Expr::cust("count(*)"))
		.from(Alias::new("dashboard_login_transactions"))
		.to_string(PostgresQueryBuilder);
	let count: i64 = sqlx::query_scalar(&count_query)
		.fetch_one(&federation.store.pool)
		.await
		.unwrap();
	assert_eq!(count, 8);
	assert_eq!(discoveries.load(Ordering::SeqCst), 1);
	// A full browser queue must be rejected before contacting an uncached issuer.
	federation.config.oidc.as_mut().unwrap().issuer =
		"http://127.0.0.1:1/realms/unavailable".into();
	let denied = api::router(federation.clone())
		.oneshot(
			Request::builder()
				.uri("/auth/login")
				.header("cookie", cookie.unwrap())
				.body(Body::empty())
				.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(denied.status(), 409);
	server.abort();
	common::cleanup(federation, &url, &schema).await;
}

// Each call names the authority inputs explicitly for this security test.
#[allow(clippy::too_many_arguments)]
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
	let expire = Query::update()
		.table(Alias::new("dashboard_registration_requests"))
		.value(
			Alias::new("expires_at"),
			Expr::cust("clock_timestamp()-interval '1 second'"),
		)
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&expire)
		.bind(first["id"].as_str().unwrap().parse::<Uuid>().unwrap())
		.execute(&federation.store.pool)
		.await
		.unwrap();
	let (_, expired) = call(
		&app,
		"GET",
		"/auth/registration",
		true,
		false,
		None,
		false,
		Value::Null,
	)
	.await;
	assert_eq!(expired["status"], "expired");
	let registration_response = app
		.clone()
		.oneshot(
			Request::builder()
				.uri("/auth/registration")
				.header("cookie", "aidash-session=fixture-session")
				.body(Body::empty())
				.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(registration_response.headers()["cache-control"], "no-store");
	let (_, listed) = call(
		&app,
		"GET",
		"/api/dashboard/registrations",
		false,
		false,
		None,
		true,
		Value::Null,
	)
	.await;
	assert_eq!(listed[0]["status"], "expired");
	let (status, fresh) = call(
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
	assert_ne!(fresh["id"], first["id"]);
	let path = format!(
		"/api/dashboard/registrations/{}/approve",
		fresh["id"].as_str().unwrap()
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
	let (_, mapping_rows) = call(
		&app,
		"GET",
		"/api/dashboard/mappings",
		false,
		false,
		None,
		true,
		Value::Null,
	)
	.await;
	let revision = mapping_rows[0]["revision"].as_i64().unwrap();
	assert_eq!(
		call(
			&app,
			"POST",
			&path,
			false,
			false,
			None,
			true,
			json!({"expected_revision":revision})
		)
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
	let (status, repeated) = call(
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
	assert_eq!(repeated["status"], "pending");
	let path = format!(
		"/api/dashboard/registrations/{}/approve",
		repeated["id"].as_str().unwrap()
	);
	let (status, restored) = call(
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
	assert_eq!(status, 200, "{restored}");
	assert_eq!(restored["id"], mapping["id"]);
	let disable_path = format!(
		"/api/dashboard/mappings/{}/disable",
		mapping["id"].as_str().unwrap()
	);
	assert_eq!(
		call(
			&app,
			"POST",
			&disable_path,
			false,
			false,
			None,
			true,
			json!({"expected_revision":revision})
		)
		.await
		.0,
		409
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
		200
	);
	let grant_path = format!("/api/dashboard/identities/{identity_id}/operator-grant");
	let (status, grant) = call(
		&app,
		"POST",
		&grant_path,
		false,
		false,
		None,
		true,
		json!({"enabled":true,"expected_revision":0}),
	)
	.await;
	assert_eq!(status, 200, "{grant}");
	assert_eq!(grant["revision"], 1);
	assert_eq!(
		call(
			&app,
			"POST",
			&grant_path,
			false,
			false,
			None,
			true,
			json!({"enabled":false,"expected_revision":0})
		)
		.await
		.0,
		409
	);
	for path in [
		"/api/marketplace",
		"/api/transactions",
		"/api/generation/acme/policies/research",
		"/api/generation/acme/requests/00000000-0000-0000-0000-000000000001/control",
		"/api/workspaces/00000000-0000-0000-0000-000000000001/semantic/index",
	] {
		assert_ne!(
			call(
				&app,
				"POST",
				path,
				true,
				true,
				Some("operator"),
				false,
				Value::Null
			)
			.await
			.0,
			403,
			"operator route {path} must reach its handler"
		);
	}
	assert_eq!(
		call(
			&app,
			"POST",
			"/api/generation/acme/tasks/00000000-0000-0000-0000-000000000001/assign",
			true,
			true,
			Some("operator"),
			false,
			Value::Null
		)
		.await
		.0,
		403
	);
	let (status, revoked) = call(
		&app,
		"POST",
		&grant_path,
		false,
		false,
		None,
		true,
		json!({"enabled":false,"expected_revision":1}),
	)
	.await;
	assert_eq!(status, 200, "{revoked}");
	assert_eq!(revoked["revision"], 2);
	assert_eq!(
		call(
			&app,
			"POST",
			&grant_path,
			false,
			false,
			None,
			true,
			json!({"enabled":true,"expected_revision":1})
		)
		.await
		.0,
		409
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
	let expire_session = Query::update()
		.table(Alias::new("dashboard_sessions"))
		.value(
			Alias::new("expires_at"),
			Expr::cust("clock_timestamp()-interval '1 second'"),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&expire_session)
		.execute(&federation.store.pool)
		.await
		.unwrap();
	let (stop, stopping) = tokio::sync::watch::channel(false);
	let refresh = tokio::spawn(aidash::dashboard_auth::refresh_active(
		federation.clone(),
		stopping,
	));
	let count_query = Query::select()
		.expr(Expr::cust("count(*)"))
		.from(Alias::new("dashboard_sessions"))
		.to_string(PostgresQueryBuilder);
	tokio::time::timeout(std::time::Duration::from_secs(5), async {
		loop {
			let count: i64 = sqlx::query_scalar(&count_query)
				.fetch_one(&federation.store.pool)
				.await
				.unwrap();
			if count == 0 {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(20)).await;
		}
	})
	.await
	.unwrap();
	stop.send(true).unwrap();
	refresh.await.unwrap().unwrap();
	common::cleanup(federation, &url, &schema).await;
}
