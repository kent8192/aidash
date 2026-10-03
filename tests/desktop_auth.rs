mod common;
use aidash::{api, config::OidcConfig, federation::Federation};
use axum::{
	Router,
	body::{Body, to_bytes},
	http::{Request, StatusCode},
	response::Response,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use common::{TestEnvironment, test_environment};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

const COOKIE: &str = "aidash-session=desktop-browser-fixture; aidash-csrf=desktop-csrf";
const ORIGIN: &str = "http://127.0.0.1:8080";

struct BrowserServer(tokio::task::JoinHandle<()>);
impl Drop for BrowserServer {
	fn drop(&mut self) {
		self.0.abort();
	}
}

#[rstest::rstest]
#[tokio::test]
#[ignore = "requires web npm dependencies and Chromium; run scripts/test-desktop-browser.sh"]
async fn desktop_consent_in_chromium(
	#[future(awt)]
	#[from(test_environment)]
	environment: std::sync::Arc<TestEnvironment>,
) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let origin = format!("http://{}", listener.local_addr().unwrap());
	let (f, app, url, schema) = setup(&environment, &origin).await;
	let server = BrowserServer(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	}));
	let output = tokio::time::timeout(
		std::time::Duration::from_secs(90),
		tokio::process::Command::new("node")
			.arg(
				std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
					.join("web/desktop-tests/consent-browser.mjs"),
			)
			.arg(origin)
			.kill_on_drop(true)
			.output(),
	)
	.await
	.expect("Chromium consent regression must finish")
	.expect("Node must be installed");
	drop(server);
	common::cleanup(f, &url, &schema).await;
	assert!(
		output.status.success(),
		"{}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
}

async fn request(
	app: &Router,
	method: &str,
	path: &str,
	headers: &[(&str, &str)],
	body: String,
) -> Response {
	let mut request = Request::builder().method(method).uri(path);
	for (name, value) in headers {
		request = request.header(*name, *value);
	}
	app.clone()
		.oneshot(request.body(Body::from(body)).unwrap())
		.await
		.unwrap()
}
async fn json_response(response: Response, expected: u16) -> Value {
	let status = response.status().as_u16();
	let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
	assert_eq!(status, expected, "{}", String::from_utf8_lossy(&bytes));
	serde_json::from_slice(&bytes).unwrap()
}
async fn post(app: &Router, path: &str, body: Value) -> Response {
	request(
		app,
		"POST",
		path,
		&[("content-type", "application/json")],
		body.to_string(),
	)
	.await
}
async fn setup(
	environment: &TestEnvironment,
	origin: &str,
) -> (Federation, Router, String, String) {
	let (mut f, url, schema) = common::setup(environment).await;
	f.config.oidc = Some(OidcConfig {
		issuer: "https://accounts.google.com".into(),
		client_id: "fixture".into(),
		client_secret: "server-only-fixture".into(),
		public_origin: origin.into(),
		keycloak_admin_url: String::new(),
		status_client_id: String::new(),
		status_client_secret: String::new(),
		session_absolute_seconds: 43_200,
		session_idle_seconds: 1_800,
	});
	let identity = Uuid::new_v4();
	let query = Query::insert()
		.into_table(Alias::new("dashboard_identities"))
		.columns(["id", "issuer", "subject", "last_valid_at"].map(Alias::new))
		.values_panic([
			Expr::val(identity).into(),
			Expr::val("https://accounts.google.com").into(),
			Expr::val("desktop-fixture").into(),
			Expr::cust("clock_timestamp()"),
		])
		.to_string(PostgresQueryBuilder);
	sqlx::query(&query).execute(&f.store.pool).await.unwrap();
	let query = Query::insert()
		.into_table(Alias::new("dashboard_operator_grants"))
		.columns([Alias::new("identity_id")])
		.values_panic([Expr::val(identity).into()])
		.to_string(PostgresQueryBuilder);
	sqlx::query(&query).execute(&f.store.pool).await.unwrap();
	let query = Query::insert()
		.into_table(Alias::new("dashboard_sessions"))
		.columns(
			[
				"id",
				"identity_id",
				"token_hash",
				"csrf_hash",
				"created_at",
				"last_activity_at",
				"expires_at",
			]
			.map(Alias::new),
		)
		.values_panic([
			Expr::val(Uuid::new_v4()).into(),
			Expr::val(identity).into(),
			Expr::val(Sha256::digest(b"desktop-browser-fixture").to_vec()).into(),
			Expr::val(Sha256::digest(b"desktop-csrf").to_vec()).into(),
			Expr::cust("clock_timestamp()"),
			Expr::cust("clock_timestamp()"),
			Expr::cust("clock_timestamp()+interval '12 hours'"),
		])
		.to_string(PostgresQueryBuilder);
	sqlx::query(&query).execute(&f.store.pool).await.unwrap();
	let update = Query::update()
		.table(Alias::new("dashboard_sessions"))
		.value(Alias::new("provider_sid"), "fixture-provider-session")
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update).execute(&f.store.pool).await.unwrap();
	let app = api::router_with_settings(
		f.clone(),
		aidash::http::Settings {
			auth_burst: 10000,
			..Default::default()
		},
	)
	.layer(axum::Extension(axum::extract::ConnectInfo(
		"127.0.0.1:12345".parse::<std::net::SocketAddr>().unwrap(),
	)));
	(f, app, url, schema)
}
async fn login(app: &Router) -> Value {
	let verifier = "a".repeat(64);
	let state = "b".repeat(64);
	let redirect = "http://127.0.0.1:43217/callback";
	let start=json_response(post(app,"/auth/desktop/start",json!({"state":state,"code_challenge":URL_SAFE_NO_PAD.encode(Sha256::digest(&verifier)),"redirect_uri":redirect})).await,200).await;
	let authorize = start["authorization_url"]
		.as_str()
		.unwrap()
		.strip_prefix(ORIGIN)
		.unwrap();
	let without_cookie = request(app, "GET", authorize, &[], String::new()).await;
	assert_eq!(without_cookie.status(), StatusCode::SEE_OTHER);
	assert!(
		without_cookie.headers()["location"]
			.to_str()
			.unwrap()
			.starts_with("/auth/login?return_to=")
	);
	let consent = request(app, "GET", authorize, &[("cookie", COOKIE)], String::new()).await;
	assert_eq!(consent.status(), StatusCode::OK);
	assert_eq!(consent.headers()["cache-control"], "no-store");
	assert_eq!(
		consent.headers()["content-security-policy"],
		"default-src 'none'; form-action 'self' http://127.0.0.1:43217; frame-ancestors 'none'; base-uri 'none'"
	);
	let id = authorize.split("request=").nth(1).unwrap();
	let body = format!("request={id}&csrf=desktop-csrf");
	for origin in ["https://evil.example", "null"] {
		let denied = request(
			app,
			"POST",
			"/auth/desktop/authorize",
			&[
				("cookie", COOKIE),
				("content-type", "application/x-www-form-urlencoded"),
				("origin", origin),
			],
			body.clone(),
		)
		.await;
		assert_eq!(denied.status(), StatusCode::FORBIDDEN);
	}
	let approved = request(
		app,
		"POST",
		"/auth/desktop/authorize",
		&[
			("cookie", COOKIE),
			("content-type", "application/x-www-form-urlencoded"),
			("origin", ORIGIN),
		],
		body,
	)
	.await;
	assert_eq!(approved.status(), StatusCode::SEE_OTHER);
	let callback = reqwest::Url::parse(approved.headers()["location"].to_str().unwrap()).unwrap();
	let pairs = callback
		.query_pairs()
		.collect::<std::collections::HashMap<_, _>>();
	assert_eq!(pairs.len(), 2, "callback must contain only code and state");
	assert_eq!(pairs["state"], state);
	let body =
		json!({"code":pairs["code"],"state":state,"verifier":verifier,"redirect_uri":redirect});
	let mut wrong = body.clone();
	wrong["verifier"] = json!("c".repeat(64));
	assert_eq!(
		post(app, "/auth/desktop/exchange", wrong).await.status(),
		StatusCode::UNAUTHORIZED
	);
	let result = json_response(post(app, "/auth/desktop/exchange", body.clone()).await, 200).await;
	assert_eq!(
		post(app, "/auth/desktop/exchange", body).await.status(),
		StatusCode::UNAUTHORIZED,
		"handoff is single-use"
	);
	assert_eq!(result["expires_in"], 300);
	result
}
async fn bearer(app: &Router, method: &str, path: &str, token: &Value) -> Response {
	request(
		app,
		method,
		path,
		&[
			(
				"authorization",
				&format!("Bearer {}", token.as_str().unwrap()),
			),
			("x-aidash-context", "operator"),
		],
		String::new(),
	)
	.await
}
#[rstest::rstest]
#[tokio::test]
async fn desktop_handoff_rotation_recovery_and_revocation_preserve_web_sessions(
	#[future(awt)]
	#[from(test_environment)]
	environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, app, url, schema) = setup(&environment, ORIGIN).await;
	let tokens = login(&app).await;
	let session = json_response(
		bearer(&app, "GET", "/auth/session", &tokens["access_token"]).await,
		200,
	)
	.await;
	assert_eq!(session["operator"], true);
	let sid_query = Query::select()
		.column(Alias::new("provider_sid"))
		.from(Alias::new("dashboard_sessions"))
		.and_where(
			Expr::col(Alias::new("id"))
				.eq(Uuid::parse_str(session["id"].as_str().unwrap()).unwrap()),
		)
		.to_string(PostgresQueryBuilder);
	let sid: Option<String> = sqlx::query_scalar(&sid_query)
		.fetch_one(&f.store.pool)
		.await
		.unwrap();
	assert_eq!(
		sid.as_deref(),
		Some("fixture-provider-session"),
		"provider backchannel revocation retains its session binding"
	);
	assert_eq!(
		bearer(&app, "GET", "/api/state", &tokens["access_token"])
			.await
			.status(),
		StatusCode::OK
	);
	// A desktop access token is never accepted as a cookie session.
	assert_eq!(
		request(
			&app,
			"GET",
			"/auth/session",
			&[(
				"cookie",
				&format!(
					"aidash-session={}",
					tokens["access_token"].as_str().unwrap()
				)
			)],
			String::new()
		)
		.await
		.status(),
		StatusCode::UNAUTHORIZED
	);
	assert_eq!(
		request(
			&app,
			"POST",
			"/auth/activity",
			&[("cookie", COOKIE)],
			String::new()
		)
		.await
		.status(),
		StatusCode::FORBIDDEN
	);
	assert_eq!(
		bearer(&app, "POST", "/auth/activity", &tokens["access_token"])
			.await
			.status(),
		StatusCode::NO_CONTENT
	);
	let activity = Query::select()
		.column(Alias::new("last_activity_at"))
		.from(Alias::new("dashboard_sessions"))
		.and_where(
			Expr::col(Alias::new("id"))
				.eq(Uuid::parse_str(session["id"].as_str().unwrap()).unwrap()),
		)
		.to_string(PostgresQueryBuilder);
	let before: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(&activity)
		.fetch_one(&f.store.pool)
		.await
		.unwrap();
	let next = format!("aidash_refresh_{}", "d".repeat(64));
	let renewal = json!({"refresh_token":tokens["refresh_token"],"next_token":next});
	let rotated = json_response(
		post(&app, "/auth/desktop/refresh", renewal.clone()).await,
		200,
	)
	.await;
	let recovered = json_response(
		post(&app, "/auth/desktop/refresh", renewal.clone()).await,
		200,
	)
	.await;
	assert_eq!(
		rotated["refresh_token"], recovered["refresh_token"],
		"lost response recovery uses the prepared successor"
	);
	assert_eq!(
		bearer(&app, "GET", "/auth/session", &tokens["access_token"])
			.await
			.status(),
		StatusCode::UNAUTHORIZED
	);
	assert_eq!(
		bearer(&app, "GET", "/auth/session", &recovered["access_token"])
			.await
			.status(),
		StatusCode::OK
	);
	let after: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(&activity)
		.fetch_one(&f.store.pool)
		.await
		.unwrap();
	assert_eq!(before, after, "refresh does not manufacture user activity");
	let mut replay = renewal;
	replay["next_token"] = json!(format!("aidash_refresh_{}", "e".repeat(64)));
	assert_eq!(
		post(&app, "/auth/desktop/refresh", replay).await.status(),
		StatusCode::UNAUTHORIZED
	);
	assert_eq!(
		bearer(&app, "GET", "/auth/session", &recovered["access_token"])
			.await
			.status(),
		StatusCode::UNAUTHORIZED,
		"reuse revokes the family"
	);
	assert_eq!(
		request(
			&app,
			"GET",
			"/auth/session",
			&[("cookie", COOKIE)],
			String::new()
		)
		.await
		.status(),
		StatusCode::OK,
		"desktop revocation preserves the browser session"
	);
	let current = login(&app).await;
	assert_eq!(
		post(
			&app,
			"/auth/desktop/revoke",
			json!({"refresh_token":current["refresh_token"]})
		)
		.await
		.status(),
		StatusCode::NO_CONTENT
	);
	assert_eq!(
		bearer(&app, "GET", "/auth/session", &current["access_token"])
			.await
			.status(),
		StatusCode::UNAUTHORIZED
	);
	let all = login(&app).await;
	assert_eq!(
		bearer(&app, "POST", "/auth/logout-all", &all["access_token"])
			.await
			.status(),
		StatusCode::NO_CONTENT
	);
	assert_eq!(
		request(
			&app,
			"GET",
			"/auth/session",
			&[("cookie", COOKIE)],
			String::new()
		)
		.await
		.status(),
		StatusCode::UNAUTHORIZED
	);
	assert_eq!(post(&app,"/auth/desktop/refresh",json!({"refresh_token":all["refresh_token"],"next_token":format!("aidash_refresh_{}","f".repeat(64))})).await.status(),StatusCode::UNAUTHORIZED);
	common::cleanup(f, &url, &schema).await;
}
#[rstest::rstest]
#[tokio::test]
async fn desktop_expiry_and_cors_boundaries(
	#[future(awt)]
	#[from(test_environment)]
	environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, app, url, schema) = setup(&environment, ORIGIN).await;
	for origin in [
		"tauri://localhost",
		"http://tauri.localhost",
		"https://evil.example",
	] {
		let response = request(
			&app,
			"OPTIONS",
			"/api/state",
			&[
				("origin", origin),
				("access-control-request-method", "GET"),
				(
					"access-control-request-headers",
					"authorization,x-aidash-context,last-event-id",
				),
			],
			String::new(),
		)
		.await;
		if origin.ends_with("localhost") {
			assert_eq!(response.headers()["access-control-allow-origin"], origin);
		} else {
			assert!(
				response
					.headers()
					.get("access-control-allow-origin")
					.is_none()
			);
		}
		assert!(
			response
				.headers()
				.get("access-control-allow-credentials")
				.is_none()
		);
	}
	assert_eq!(post(&app,"/auth/desktop/start",json!({"state":"s".repeat(64),"code_challenge":URL_SAFE_NO_PAD.encode(Sha256::digest("v".repeat(64))),"redirect_uri":"https://evil.example/callback"})).await.status(),StatusCode::BAD_REQUEST);
	for (column, value) in [
		("access_expires_at", "clock_timestamp()-interval '1 second'"),
		("last_activity_at", "clock_timestamp()-interval '31 days'"),
		("expires_at", "clock_timestamp()-interval '1 second'"),
	] {
		let tokens = login(&app).await;
		let update = Query::update()
			.table(Alias::new("dashboard_sessions"))
			.value(Alias::new(column), Expr::cust(value))
			.and_where(
				Expr::col(Alias::new("token_hash"))
					.eq(Sha256::digest(tokens["access_token"].as_str().unwrap()).to_vec()),
			)
			.to_string(PostgresQueryBuilder);
		sqlx::query(&update).execute(&f.store.pool).await.unwrap();
		assert_eq!(
			bearer(&app, "GET", "/auth/session", &tokens["access_token"])
				.await
				.status(),
			StatusCode::UNAUTHORIZED
		);
		let refresh = post(&app,"/auth/desktop/refresh",json!({"refresh_token":tokens["refresh_token"],"next_token":format!("aidash_refresh_{}",Uuid::new_v4().simple().to_string().repeat(2))})).await;
		assert_eq!(
			refresh.status(),
			if column == "access_expires_at" {
				StatusCode::OK
			} else {
				StatusCode::UNAUTHORIZED
			}
		);
	}
	common::cleanup(f, &url, &schema).await;
}
