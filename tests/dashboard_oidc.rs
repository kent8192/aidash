mod common;

use aidash::{
	api,
	authorization::{Authorization, policy::PolicyBundle},
	config::OidcConfig,
};
use axum::{
	Json, Router,
	body::Body,
	http::Request,
	routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::{StreamExt, future::join_all};
use sea_orm::sea_query::{Alias, Expr, LockType, PostgresQueryBuilder, Query};
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
	let discovery_started = Arc::new(tokio::sync::Notify::new());
	let started = discovery_started.clone();
	let discovery_release = Arc::new(tokio::sync::Notify::new());
	let release = discovery_release.clone();
	let key_fetches = Arc::new(AtomicUsize::new(0));
	let key_counter = key_fetches.clone();
	let fixture = Router::new()
		.route(
			"/realms/test/.well-known/openid-configuration",
			get(move || {
				let issuer = metadata_issuer.clone();
				discovery_counter.fetch_add(1, Ordering::SeqCst);
				let started = started.clone();
				let release = release.clone();
				async move {
					started.notify_one();
					release.notified().await;
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
			get(move || {
				key_counter.fetch_add(1, Ordering::SeqCst);
				async { Json(json!({"keys": []})) }
			}),
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
	let first_app = app.clone();
	let first_request = tokio::spawn(async move {
		first_app
			.oneshot(
				Request::builder()
					.uri("/auth/login")
					.body(Body::empty())
					.unwrap(),
			)
			.await
			.unwrap()
	});
	discovery_started.notified().await;
	let mut admission_probe = federation.store.pool.begin().await.unwrap();
	let admission_lock_available: bool =
		sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(71003204)")
			.fetch_one(&mut *admission_probe)
			.await
			.unwrap();
	assert!(
		admission_lock_available,
		"login admission lock stayed held during discovery"
	);
	admission_probe.commit().await.unwrap();
	discovery_release.notify_one();
	let first = first_request.await.unwrap();
	assert_eq!(first.status(), 307);
	let cookie = first.headers()["set-cookie"]
		.to_str()
		.unwrap()
		.split(';')
		.next()
		.unwrap()
		.to_owned();
	let responses = join_all((0..15).map(|_| {
		let app = app.clone();
		let cookie = cookie.clone();
		async move {
			app.oneshot(
				Request::builder()
					.uri("/auth/login")
					.header("cookie", cookie)
					.body(Body::empty())
					.unwrap(),
			)
			.await
			.unwrap()
			.status()
			.as_u16()
		}
	}))
	.await;
	assert_eq!(
		responses.iter().filter(|&&status| status == 307).count(),
		7,
		"{responses:?}"
	);
	assert_eq!(
		responses.iter().filter(|&&status| status == 409).count(),
		8,
		"{responses:?}"
	);
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
	let initial_key_fetches = key_fetches.load(Ordering::SeqCst);
	let header = URL_SAFE_NO_PAD.encode(json!({"alg":"RS256","kid":"unknown"}).to_string());
	let payload = URL_SAFE_NO_PAD.encode("{}");
	let token = format!("{header}.{payload}.signature");
	for _ in 0..3 {
		let response = app
			.clone()
			.oneshot(
				Request::builder()
					.method("POST")
					.uri("/auth/backchannel-logout")
					.header("content-type", "application/x-www-form-urlencoded")
					.body(Body::from(format!("logout_token={token}")))
					.unwrap(),
			)
			.await
			.unwrap();
		assert_eq!(response.status(), 400);
	}
	assert_eq!(discoveries.load(Ordering::SeqCst), 1);
	assert_eq!(key_fetches.load(Ordering::SeqCst), initial_key_fetches + 1);
	// A full browser queue must be rejected before contacting an uncached issuer.
	federation.config.oidc.as_mut().unwrap().issuer =
		"http://127.0.0.1:1/realms/unavailable".into();
	let denied = api::router(federation.clone())
		.oneshot(
			Request::builder()
				.uri("/auth/login")
				.header("cookie", cookie)
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
	let mut changed_issuer = federation.clone();
	changed_issuer.config.oidc.as_mut().unwrap().issuer =
		"http://127.0.0.1:18099/realms/other".into();
	assert_eq!(
		call(
			&api::router(changed_issuer),
			"GET",
			"/auth/session",
			true,
			false,
			None,
			false,
			Value::Null,
		)
		.await
		.0,
		403
	);
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
	assert_eq!(listed, json!([]));
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
	let insert_old = Query::insert()
		.into_table(Alias::new("dashboard_registration_requests"))
		.columns(["id", "identity_id", "status", "created_at", "expires_at"].map(Alias::new))
		.values_panic([
			Expr::cust("$1"),
			Expr::cust("$2"),
			Expr::value("rejected"),
			Expr::cust("clock_timestamp()"),
			Expr::cust("clock_timestamp()+interval '1 day'"),
		])
		.to_string(PostgresQueryBuilder);
	for _ in 0..201 {
		sqlx::query(&insert_old)
			.bind(Uuid::new_v4())
			.bind(identity_id)
			.execute(&federation.store.pool)
			.await
			.unwrap();
	}
	let (status, actionable) = call(
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
	assert_eq!(status, 200);
	assert_eq!(actionable, json!([fresh]));
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
	let credential_query = Query::select()
		.column(Alias::new("credential_id"))
		.from(Alias::new("dashboard_mappings"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	let credential_id: Uuid = sqlx::query_scalar(&credential_query)
		.bind(mapping["id"].as_str().unwrap().parse::<Uuid>().unwrap())
		.fetch_one(&federation.store.pool)
		.await
		.unwrap();
	assert_eq!(
		call(
			&app,
			"POST",
			&format!("/api/authorization/acme/credentials/{credential_id}/revoke"),
			false,
			false,
			None,
			true,
			Value::Null
		)
		.await
		.0,
		404
	);
	let (_, generic_credentials) = call(
		&app,
		"GET",
		"/api/authorization/acme/credentials",
		false,
		false,
		None,
		true,
		Value::Null,
	)
	.await;
	assert!(
		!generic_credentials
			.to_string()
			.contains(&credential_id.to_string())
	);
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
	let mut worker = federation.store.pool.begin().await.unwrap();
	let lock_credential = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("authorization_credentials"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
		.lock(LockType::Share)
		.to_string(PostgresQueryBuilder);
	sqlx::query_scalar::<_, Uuid>(&lock_credential)
		.bind(credential_id)
		.fetch_one(&mut *worker)
		.await
		.unwrap();
	let disable_app = app.clone();
	let disable_path = path.clone();
	let disable = tokio::spawn(async move {
		call(
			&disable_app,
			"POST",
			&disable_path,
			false,
			false,
			None,
			true,
			json!({"expected_revision":revision}),
		)
		.await
		.0
	});
	tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	let lock_mapping = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("dashboard_mappings"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
		.lock(LockType::Share)
		.to_string(PostgresQueryBuilder);
	tokio::time::timeout(
		std::time::Duration::from_secs(2),
		sqlx::query_scalar::<_, Uuid>(&lock_mapping)
			.bind(mapping["id"].as_str().unwrap().parse::<Uuid>().unwrap())
			.fetch_one(&mut *worker),
	)
	.await
	.unwrap()
	.unwrap();
	worker.commit().await.unwrap();
	assert_eq!(
		tokio::time::timeout(std::time::Duration::from_secs(5), disable)
			.await
			.unwrap()
			.unwrap(),
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
		"/api/workspaces/00000000-0000-0000-0000-000000000001/semantic/search",
		"/api/workspaces/00000000-0000-0000-0000-000000000001/semantic/entries",
		"/api/workspaces/00000000-0000-0000-0000-000000000001/semantic/entries/00000000-0000-0000-0000-000000000002/reindex",
		"/api/remote",
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
	assert_ne!(
		call(
			&app,
			"DELETE",
			"/api/workspaces/00000000-0000-0000-0000-000000000001/semantic/entries/00000000-0000-0000-0000-000000000002",
			true,
			true,
			Some("operator"),
			false,
			Value::Null,
		)
		.await
		.0,
		403
	);
	let stream_response = app
		.clone()
		.oneshot(
			Request::builder()
				.uri("/api/events/stream?after=-1")
				.header("cookie", "aidash-session=fixture-session")
				.header("x-aidash-context", "operator")
				.body(Body::empty())
				.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(stream_response.status(), 200);
	let mut operator_stream = stream_response.into_body().into_data_stream();
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
	assert!(
		tokio::time::timeout(std::time::Duration::from_secs(7), operator_stream.next())
			.await
			.unwrap()
			.is_none()
	);
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
	for index in 0..205 {
		sqlx::query(&insert_identity)
			.bind(Uuid::new_v4())
			.bind("http://127.0.0.1:18099/realms/test")
			.bind(format!("paged-{index:03}"))
			.execute(&federation.store.pool)
			.await
			.unwrap();
	}
	let (status, first_page) = call(
		&app,
		"GET",
		"/api/dashboard/identities?offset=0",
		false,
		false,
		None,
		true,
		Value::Null,
	)
	.await;
	assert_eq!(status, 200);
	assert_eq!(first_page.as_array().unwrap().len(), 200);
	let (status, second_page) = call(
		&app,
		"GET",
		"/api/dashboard/identities?offset=200",
		false,
		false,
		None,
		true,
		Value::Null,
	)
	.await;
	assert_eq!(status, 200);
	assert_eq!(second_page.as_array().unwrap().len(), 6);
	let first_ids: std::collections::HashSet<_> = first_page
		.as_array()
		.unwrap()
		.iter()
		.map(|identity| identity["id"].as_str().unwrap())
		.collect();
	assert!(
		second_page
			.as_array()
			.unwrap()
			.iter()
			.all(|identity| !first_ids.contains(identity["id"].as_str().unwrap()))
	);
	let insert_credential = Query::insert()
		.into_table(Alias::new("authorization_credentials"))
		.columns(
			[
				"id",
				"tenant",
				"subject",
				"token_hash",
				"expires_at",
				"issued_by",
			]
			.map(Alias::new),
		)
		.values_panic([
			Expr::cust("$1"),
			Expr::cust("$2"),
			Expr::cust("$3"),
			Expr::cust("$4"),
			Expr::cust("clock_timestamp()+interval '12 hours'"),
			Expr::cust("$5"),
		])
		.to_string(PostgresQueryBuilder);
	let insert_mapping = Query::insert()
		.into_table(Alias::new("dashboard_mappings"))
		.columns(["id", "identity_id", "tenant", "subject", "credential_id"].map(Alias::new))
		.values_panic([
			Expr::cust("$1"),
			Expr::cust("$2"),
			Expr::cust("$3"),
			Expr::cust("$4"),
			Expr::cust("$5"),
		])
		.to_string(PostgresQueryBuilder);
	for index in 0..205 {
		let mapping_id = Uuid::new_v4();
		let credential_id = Uuid::new_v4();
		let subject = format!("paged-{index:03}");
		sqlx::query(&insert_credential)
			.bind(credential_id)
			.bind("acme")
			.bind(&subject)
			.bind(Sha256::digest(format!("mapping-token-{index}")).to_vec())
			.bind("operator")
			.execute(&federation.store.pool)
			.await
			.unwrap();
		sqlx::query(&insert_mapping)
			.bind(mapping_id)
			.bind(identity_id)
			.bind("acme")
			.bind(subject)
			.bind(credential_id)
			.execute(&federation.store.pool)
			.await
			.unwrap();
	}
	let (status, first_mapping_page) = call(
		&app,
		"GET",
		"/api/dashboard/mappings?offset=0",
		false,
		false,
		None,
		true,
		Value::Null,
	)
	.await;
	assert_eq!(status, 200);
	assert_eq!(first_mapping_page.as_array().unwrap().len(), 200);
	let (status, second_mapping_page) = call(
		&app,
		"GET",
		"/api/dashboard/mappings?offset=200",
		false,
		false,
		None,
		true,
		Value::Null,
	)
	.await;
	assert_eq!(status, 200);
	assert_eq!(second_mapping_page.as_array().unwrap().len(), 6);
	let first_mapping_ids: std::collections::HashSet<_> = first_mapping_page
		.as_array()
		.unwrap()
		.iter()
		.map(|mapping| mapping["id"].as_str().unwrap())
		.collect();
	assert!(
		second_mapping_page
			.as_array()
			.unwrap()
			.iter()
			.all(|mapping| !first_mapping_ids.contains(mapping["id"].as_str().unwrap()))
	);
	sqlx::query(&insert_session)
		.bind(Uuid::new_v4())
		.bind(Sha256::digest(b"issuer-change-session").to_vec())
		.bind(Sha256::digest(b"issuer-change-csrf").to_vec())
		.bind(identity_id)
		.execute(&federation.store.pool)
		.await
		.unwrap();
	let mut changed_issuer = federation.clone();
	changed_issuer.config.oidc.as_mut().unwrap().issuer =
		"http://127.0.0.1:18099/realms/other".into();
	let (stop, stopping) = tokio::sync::watch::channel(false);
	let refresh = tokio::spawn(aidash::dashboard_auth::refresh_active(
		changed_issuer,
		stopping,
	));
	let disabled = Query::select()
		.column(Alias::new("disabled_at"))
		.from(Alias::new("dashboard_identities"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	tokio::time::timeout(std::time::Duration::from_secs(5), async {
		loop {
			let disabled_at: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(&disabled)
				.bind(identity_id)
				.fetch_one(&federation.store.pool)
				.await
				.unwrap();
			if disabled_at.is_some() {
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

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn older_negative_status_cannot_revoke_a_newer_valid_session() {
	let (mut federation, url, schema) = common::setup().await;
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let issuer = format!("http://{}/realms/test", listener.local_addr().unwrap());
	let checks = Arc::new(AtomicUsize::new(0));
	let count = checks.clone();
	let release = Arc::new(tokio::sync::Notify::new());
	let released = release.clone();
	let fixture = Router::new()
		.route(
			"/realms/test/protocol/openid-connect/token",
			post(|| async { Json(json!({"access_token":"fixture"})) }),
		)
		.route(
			"/realms/test/admin/users/stale-user",
			get(move || {
				let index = count.fetch_add(1, Ordering::SeqCst);
				let released = released.clone();
				async move {
					if index == 0 {
						released.notified().await;
					}
					Json(json!({"id":"stale-user","enabled":index != 0}))
				}
			}),
		);
	let server = tokio::spawn(async move { axum::serve(listener, fixture).await.unwrap() });
	federation.config.oidc = Some(OidcConfig {
		issuer: issuer.clone(),
		client_id: "aidash".into(),
		client_secret: "secret".into(),
		public_origin: "http://127.0.0.1:8080".into(),
		keycloak_admin_url: format!("{issuer}/admin"),
		status_client_id: "status".into(),
		status_client_secret: "status-secret".into(),
		session_absolute_seconds: 43_200,
		session_idle_seconds: 1_800,
	});
	let identity_id = Uuid::new_v4();
	let identity = Query::insert()
		.into_table(Alias::new("dashboard_identities"))
		.columns(["id", "issuer", "subject", "last_valid_at"].map(Alias::new))
		.values_panic([
			Expr::cust("$1"),
			Expr::cust("$2"),
			Expr::value("stale-user"),
			Expr::cust("clock_timestamp()-interval '10 minutes'"),
		])
		.to_string(PostgresQueryBuilder);
	sqlx::query(&identity)
		.bind(identity_id)
		.bind(&issuer)
		.execute(&federation.store.pool)
		.await
		.unwrap();
	let session = Query::insert()
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
	sqlx::query(&session)
		.bind(Uuid::new_v4())
		.bind(Sha256::digest(b"fixture-session").to_vec())
		.bind(Sha256::digest(b"fixture-csrf").to_vec())
		.bind(identity_id)
		.execute(&federation.store.pool)
		.await
		.unwrap();
	let app = api::router(federation.clone());
	let old_app = app.clone();
	let old = tokio::spawn(async move {
		call(
			&old_app,
			"GET",
			"/auth/session",
			true,
			false,
			None,
			false,
			Value::Null,
		)
		.await
		.0
	});
	tokio::time::timeout(std::time::Duration::from_secs(5), async {
		while checks.load(Ordering::SeqCst) == 0 {
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	})
	.await
	.unwrap();
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
		200
	);
	release.notify_one();
	assert_eq!(old.await.unwrap(), 403);
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
		200
	);
	let status = Query::select()
		.columns([Alias::new("disabled_at"), Alias::new("revoked_at")])
		.from(Alias::new("dashboard_identities"))
		.join(
			sea_orm::sea_query::JoinType::InnerJoin,
			Alias::new("dashboard_sessions"),
			Expr::col((Alias::new("dashboard_identities"), Alias::new("id")))
				.equals((Alias::new("dashboard_sessions"), Alias::new("identity_id"))),
		)
		.and_where(
			Expr::col((Alias::new("dashboard_identities"), Alias::new("id"))).eq(Expr::cust("$1")),
		)
		.to_string(PostgresQueryBuilder);
	let (disabled_at, revoked_at): (
		Option<chrono::DateTime<chrono::Utc>>,
		Option<chrono::DateTime<chrono::Utc>>,
	) = sqlx::query_as(&status)
		.bind(identity_id)
		.fetch_one(&federation.store.pool)
		.await
		.unwrap();
	assert!(disabled_at.is_none() && revoked_at.is_none());
	server.abort();
	common::cleanup(federation, &url, &schema).await;
}
