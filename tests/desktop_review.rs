mod common;

use aidash::{api, config::OidcConfig, federation::Federation};
use axum::{
	Json, Router,
	body::Body,
	http::{Request, StatusCode},
	routing::{get, post},
};
use base64::{
	Engine,
	engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use common::{TestEnvironment, test_environment};
use openidconnect::{
	PrivateSigningKey,
	core::{CoreJsonWebKeySet, CoreRsaPrivateSigningKey},
};
use sea_orm::sea_query::{Alias, Expr, LockType, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{
	Arc,
	atomic::{AtomicBool, Ordering},
};
use tower::ServiceExt;
use uuid::Uuid;

const SUBJECT: &str = "desktop-review-user";
const SID: &str = "desktop-review-provider-session";
const ORIGIN: &str = "http://127.0.0.1:8080";
const PEM: &str = include_str!("fixtures/oidc/signing-test-only.pem");

struct Fixture {
	f: Federation,
	url: String,
	schema: String,
	identity: Uuid,
	browser: Uuid,
	outage: Arc<AtomicBool>,
	server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
	fn drop(&mut self) {
		self.server.abort();
	}
}
impl Fixture {
	async fn new(environment: &TestEnvironment) -> Self {
		let (mut f, url, schema) = common::setup(environment).await;
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let issuer = format!("http://{}/issuer", listener.local_addr().unwrap());
		let metadata = json!({
			"issuer": issuer, "authorization_endpoint": format!("{issuer}/authorize"),
			"token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
			"jwks_uri": format!("{issuer}/jwks"),
			"response_types_supported": ["code"], "subject_types_supported": ["public"],
			"id_token_signing_alg_values_supported": ["RS256"]
		});
		let signing = CoreRsaPrivateSigningKey::from_pem(
			PEM,
			Some(openidconnect::JsonWebKeyId::new("review-key".into())),
		)
		.unwrap();
		let keys =
			serde_json::to_value(CoreJsonWebKeySet::new(vec![signing.as_verification_key()]))
				.unwrap();
		let outage = Arc::new(AtomicBool::new(false));
		let unavailable = outage.clone();
		let provider = Router::new()
			.route(
				"/issuer/.well-known/openid-configuration",
				get(move || {
					let metadata = metadata.clone();
					async move { Json(metadata) }
				}),
			)
			.route(
				"/issuer/jwks",
				get(move || {
					let keys = keys.clone();
					async move { Json(keys) }
				}),
			)
			.route(
				"/issuer/protocol/openid-connect/token",
				post(|| async { Json(json!({"access_token": "status-fixture"})) }),
			)
			.route(
				"/issuer/admin/users/desktop-review-user",
				get(move || {
					let unavailable = unavailable.clone();
					async move {
						let status = if unavailable.load(Ordering::SeqCst) {
							StatusCode::SERVICE_UNAVAILABLE
						} else {
							StatusCode::OK
						};
						(status, Json(json!({"id": SUBJECT, "enabled": true})))
					}
				}),
			);
		let server = tokio::spawn(async move { axum::serve(listener, provider).await.unwrap() });
		f.config.oidc = Some(OidcConfig {
			issuer: issuer.clone(),
			client_id: "review-client".into(),
			client_secret: "fixture".into(),
			public_origin: ORIGIN.into(),
			keycloak_admin_url: format!("{issuer}/admin"),
			status_client_id: "status".into(),
			status_client_secret: "fixture".into(),
			session_absolute_seconds: 43200,
			session_idle_seconds: 1800,
		});
		let identity = Uuid::new_v4();
		let browser = Uuid::new_v4();
		let insert = Query::insert()
			.into_table(Alias::new("dashboard_identities"))
			.columns(["id", "issuer", "subject", "last_valid_at"].map(Alias::new))
			.values_panic([
				Expr::val(identity).into(),
				Expr::val(issuer).into(),
				Expr::val(SUBJECT).into(),
				Expr::cust("clock_timestamp()"),
			])
			.to_string(PostgresQueryBuilder);
		sqlx::query(&insert).execute(&f.store.pool).await.unwrap();
		let insert = Query::insert()
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
					"provider_sid",
				]
				.map(Alias::new),
			)
			.values_panic([
				Expr::val(browser).into(),
				Expr::val(identity).into(),
				Expr::val(Sha256::digest(b"review-browser").to_vec()).into(),
				Expr::val(Sha256::digest(b"review-csrf").to_vec()).into(),
				Expr::cust("clock_timestamp()"),
				Expr::cust("clock_timestamp()"),
				Expr::cust("clock_timestamp()+interval '12 hours'"),
				Expr::val(SID).into(),
			])
			.to_string(PostgresQueryBuilder);
		sqlx::query(&insert).execute(&f.store.pool).await.unwrap();
		Self {
			f,
			url,
			schema,
			identity,
			browser,
			outage,
			server,
		}
	}
	fn app(&self) -> Router {
		api::router_with_settings(
			self.f.clone(),
			aidash::http::Settings {
				auth_burst: 10000,
				..Default::default()
			},
		)
		.layer(axum::Extension(axum::extract::ConnectInfo(
			"127.0.0.1:12345".parse::<std::net::SocketAddr>().unwrap(),
		)))
	}
	async fn handoff(&self) -> Value {
		// Seed approved consent so the regression controls the exchange's lock order.
		let code = "c".repeat(64);
		let verifier = "v".repeat(64);
		let state = "s".repeat(64);
		let redirect = "http://127.0.0.1:43217/callback";
		let insert = Query::insert()
			.into_table(Alias::new("desktop_handoffs"))
			.columns(
				[
					"id",
					"state",
					"challenge",
					"redirect_uri",
					"origin",
					"browser_session_id",
					"code_hash",
					"expires_at",
				]
				.map(Alias::new),
			)
			.values_panic([
				Expr::val(Uuid::new_v4()).into(),
				Expr::val(&state).into(),
				Expr::val(URL_SAFE_NO_PAD.encode(Sha256::digest(&verifier))).into(),
				Expr::val(redirect).into(),
				Expr::val(ORIGIN).into(),
				Expr::val(self.browser).into(),
				Expr::val(Sha256::digest(&code).to_vec()).into(),
				Expr::cust("clock_timestamp()+interval '5 minutes'"),
			])
			.to_string(PostgresQueryBuilder);
		sqlx::query(&insert)
			.execute(&self.f.store.pool)
			.await
			.unwrap();
		json!({"code": code, "state": state, "verifier": verifier, "redirect_uri": redirect})
	}
	fn logout_token(&self, selector: &str) -> String {
		let config = self.f.config.oidc.as_ref().unwrap();
		let now = chrono::Utc::now().timestamp();
		let mut claims = json!({"iss": config.issuer, "aud": config.client_id, "iat": now, "exp": now+300, "jti": Uuid::new_v4().to_string(), "events": {"http://schemas.openid.net/event/backchannel-logout": {}}});
		if selector != "sid" {
			claims["sub"] = json!(SUBJECT);
		}
		if selector != "sub" {
			claims["sid"] = json!(SID);
		}
		let der = STANDARD
			.decode(
				PEM.lines()
					.filter(|line| !line.starts_with("-----"))
					.collect::<String>(),
			)
			.unwrap();
		let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
		header.kid = Some("review-key".into());
		jsonwebtoken::encode(
			&header,
			&claims,
			&jsonwebtoken::EncodingKey::from_rsa_der(&der),
		)
		.unwrap()
	}
	async fn wait_for_lock(&self, fragment: &str) {
		let query = Query::select()
			.expr(Expr::cust("count(*)"))
			.from(Alias::new("pg_stat_activity"))
			.and_where(Expr::col(Alias::new("application_name")).eq(&self.schema))
			.and_where(Expr::col(Alias::new("wait_event_type")).eq("Lock"))
			.and_where(Expr::col(Alias::new("query")).like(format!("%{fragment}%")))
			.to_string(PostgresQueryBuilder);
		tokio::time::timeout(std::time::Duration::from_secs(10), async {
			while sqlx::query_scalar::<_, i64>(&query)
				.fetch_one(&self.f.store.pool)
				.await
				.unwrap() == 0
			{
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("request must reach the expected database lock");
	}
	async fn cleanup(self) {
		common::cleanup(self.f.clone(), &self.url, &self.schema).await;
	}
}

async fn request(
	app: &Router,
	method: &str,
	path: &str,
	headers: &[(&str, &str)],
	body: Body,
) -> axum::response::Response {
	let mut builder = Request::builder().method(method).uri(path);
	for (name, value) in headers {
		builder = builder.header(*name, *value);
	}
	app.clone()
		.oneshot(builder.body(body).unwrap())
		.await
		.unwrap()
}
async fn request_json(app: &Router, method: &str, path: &str, input: Value) -> (u16, Value) {
	let response = request(
		app,
		method,
		path,
		&[("content-type", "application/json")],
		Body::from(input.to_string()),
	)
	.await;
	let status = response.status().as_u16();
	let bytes = axum::body::to_bytes(response.into_body(), 65536)
		.await
		.unwrap();
	(status, serde_json::from_slice(&bytes).unwrap())
}

async fn exchange(app: &Router, input: Value) -> (u16, Value) {
	request_json(app, "POST", "/auth/desktop/exchange", input).await
}
async fn logout(app: &Router, token: String) -> u16 {
	request(
		app,
		"POST",
		"/auth/backchannel-logout",
		&[("content-type", "application/x-www-form-urlencoded")],
		Body::from(format!("logout_token={token}")),
	)
	.await
	.status()
	.as_u16()
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn operator_token_with_desktop_prefix_retains_api_access(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
	#[case] oidc_enabled: bool,
) {
	let mut fixture = Fixture::new(&environment).await;
	fixture.f.config.api_token = format!("aidash_desktop_{}", "operator".repeat(10));
	if !oidc_enabled {
		fixture.f.config.oidc = None;
	}
	let app = fixture.app();
	let token = format!("Bearer {}", fixture.f.config.api_token);
	let response = request(
		&app,
		"GET",
		"/api/session",
		&[("authorization", &token)],
		Body::empty(),
	)
	.await;
	assert_eq!(response.status(), StatusCode::OK);
	let body: Value = serde_json::from_slice(
		&axum::body::to_bytes(response.into_body(), 65536)
			.await
			.unwrap(),
	)
	.unwrap();
	assert_eq!(body["access"]["kind"], "operator", "{body}");
	assert_eq!(
		request(
			&app,
			"GET",
			"/api/session",
			&[("authorization", "Bearer aidash_desktop_unknown")],
			Body::empty()
		)
		.await
		.status(),
		StatusCode::UNAUTHORIZED
	);
	fixture.cleanup().await;
}

#[rstest::rstest]
#[case::desktop_active(Some(7200), false, true)]
#[case::desktop_idle_expired(Some(1800), false, false)]
#[case::browser_idle_expired(None, false, false)]
#[case::revoked_desktop(Some(7200), true, false)]
#[tokio::test]
async fn status_refresh_observes_each_session_idle_lifetime(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
	#[case] idle: Option<i64>,
	#[case] revoked: bool,
	#[case] refreshed: bool,
) {
	let fixture = Fixture::new(&environment).await;
	let update = Query::update()
		.table(Alias::new("dashboard_sessions"))
		.value(Alias::new("desktop"), idle.is_some())
		.value(Alias::new("desktop_idle_seconds"), idle)
		.value(
			Alias::new("last_activity_at"),
			Expr::cust("clock_timestamp()-interval '1 hour'"),
		)
		.value(
			Alias::new("revoked_at"),
			Expr::val(if revoked {
				Some(chrono::Utc::now())
			} else {
				None
			}),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.execute(&fixture.f.store.pool)
		.await
		.unwrap();
	let update = Query::update()
		.table(Alias::new("dashboard_identities"))
		.value(
			Alias::new("last_valid_at"),
			Expr::cust("clock_timestamp()-interval '16 minutes'"),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.execute(&fixture.f.store.pool)
		.await
		.unwrap();
	let (stop, stopping) = tokio::sync::watch::channel(false);
	// A queued stop still allows the first refresh pass to finish.
	stop.send(true).unwrap();
	aidash::dashboard_auth::refresh_active(fixture.f.clone(), stopping)
		.await
		.unwrap();
	let query = Query::select()
		.column(Alias::new("last_valid_at"))
		.from(Alias::new("dashboard_identities"))
		.and_where(Expr::col(Alias::new("id")).eq(fixture.identity))
		.to_string(PostgresQueryBuilder);
	let checked: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(&query)
		.fetch_one(&fixture.f.store.pool)
		.await
		.unwrap();
	assert_eq!(
		checked > chrono::Utc::now() - chrono::Duration::minutes(1),
		refreshed
	);
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn saved_desktop_session_retains_outage_grace_after_browser_idle_expiry(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let fixture = Fixture::new(&environment).await;
	let app = fixture.app();
	let (status, tokens) = exchange(&app, fixture.handoff().await).await;
	assert_eq!(status, 200, "{tokens}");
	let update = Query::update()
		.table(Alias::new("dashboard_sessions"))
		.value(
			Alias::new("last_activity_at"),
			Expr::cust("clock_timestamp()-interval '1 hour'"),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.execute(&fixture.f.store.pool)
		.await
		.unwrap();
	let update = Query::update()
		.table(Alias::new("dashboard_identities"))
		.value(
			Alias::new("last_valid_at"),
			Expr::cust("clock_timestamp()-interval '16 minutes'"),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.execute(&fixture.f.store.pool)
		.await
		.unwrap();
	let (stop, stopping) = tokio::sync::watch::channel(false);
	stop.send(true).unwrap();
	aidash::dashboard_auth::refresh_active(fixture.f.clone(), stopping)
		.await
		.unwrap();
	fixture.outage.store(true, Ordering::SeqCst);
	// Advance past freshness while remaining within the existing outage grace.
	let update = Query::update()
		.table(Alias::new("dashboard_identities"))
		.value(
			Alias::new("last_valid_at"),
			Expr::col(Alias::new("last_valid_at")).sub(Expr::cust("interval '2 minutes'")),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.execute(&fixture.f.store.pool)
		.await
		.unwrap();
	let (status, body) = request_json(&app, "POST", "/auth/desktop/refresh", json!({"refresh_token": tokens["refresh_token"], "next_token": format!("aidash_refresh_{}", "n".repeat(64))})).await;
	assert_eq!(status, 200, "{body}");
	fixture.cleanup().await;
}

#[rstest::rstest]
#[case("sid")]
#[case("sub")]
#[case("both")]
#[tokio::test]
async fn backchannel_logout_revokes_a_concurrently_issued_desktop_session(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
	#[case] selector: &str,
) {
	let fixture = Fixture::new(&environment).await;
	let input = fixture.handoff().await;
	let app = fixture.app();
	let other_session = Uuid::new_v4();
	let source = Query::select()
		.expr(Expr::val(other_session))
		.column(Alias::new("identity_id"))
		.expr(Expr::val(
			Sha256::digest(b"other-provider-session").to_vec(),
		))
		.columns(["csrf_hash", "created_at", "last_activity_at", "expires_at"].map(Alias::new))
		.expr(Expr::val("other-provider-session"))
		.from(Alias::new("dashboard_sessions"))
		.and_where(Expr::col(Alias::new("id")).eq(fixture.browser))
		.to_owned();
	let insert = Query::insert()
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
				"provider_sid",
			]
			.map(Alias::new),
		)
		.select_from(source)
		.unwrap()
		.to_string(PostgresQueryBuilder);
	sqlx::query(&insert)
		.execute(&fixture.f.store.pool)
		.await
		.unwrap();
	let mut tx = fixture.f.store.pool.begin().await.unwrap();
	// The exchange obtains identity SHARE before waiting for this browser row.
	let lock = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("dashboard_sessions"))
		.and_where(Expr::col(Alias::new("id")).eq(fixture.browser))
		.lock(LockType::Update)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&lock).fetch_one(&mut *tx).await.unwrap();
	let issuing = tokio::spawn({
		let app = app.clone();
		async move { exchange(&app, input).await }
	});
	fixture.wait_for_lock("dashboard_sessions").await;
	let revoking = tokio::spawn({
		let app = app.clone();
		let token = fixture.logout_token(selector);
		async move { logout(&app, token).await }
	});
	fixture.wait_for_lock("dashboard_identities").await;
	tx.commit().await.unwrap();
	let (status, tokens) = issuing.await.unwrap();
	assert_eq!(status, 200, "{tokens}");
	assert_eq!(revoking.await.unwrap(), 200);
	let select = Query::select()
		.column(Alias::new("revoked_at"))
		.from(Alias::new("dashboard_sessions"))
		.and_where(Expr::col(Alias::new("id")).eq(other_session))
		.to_string(PostgresQueryBuilder);
	let revoked: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(&select)
		.fetch_one(&fixture.f.store.pool)
		.await
		.unwrap();
	assert_eq!(
		revoked.is_some(),
		selector == "sub",
		"sid logout preserves other provider sessions"
	);
	assert_eq!(
		request(
			&app,
			"GET",
			"/auth/session",
			&[(
				"authorization",
				&format!("Bearer {}", tokens["access_token"].as_str().unwrap())
			)],
			Body::empty()
		)
		.await
		.status(),
		StatusCode::UNAUTHORIZED
	);
	let (status, _) = request_json(&app, "POST", "/auth/desktop/refresh", json!({"refresh_token": tokens["refresh_token"], "next_token": format!("aidash_refresh_{}", "n".repeat(64))})).await;
	assert_eq!(status, 401);
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn handoff_cannot_issue_after_backchannel_logout_wins_identity_lock(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let fixture = Fixture::new(&environment).await;
	let input = fixture.handoff().await;
	let app = fixture.app();
	let mut tx = fixture.f.store.pool.begin().await.unwrap();
	let lock = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("dashboard_sessions"))
		.and_where(Expr::col(Alias::new("id")).eq(fixture.browser))
		.lock(LockType::Share)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&lock).fetch_one(&mut *tx).await.unwrap();
	let revoking = tokio::spawn({
		let app = app.clone();
		let token = fixture.logout_token("both");
		async move { logout(&app, token).await }
	});
	fixture.wait_for_lock("UPDATE \"dashboard_sessions\"").await;
	let issuing = tokio::spawn({
		let app = app.clone();
		async move { exchange(&app, input).await }
	});
	fixture.wait_for_lock("dashboard_identities").await;
	tx.commit().await.unwrap();
	assert_eq!(revoking.await.unwrap(), 200);
	assert_eq!(issuing.await.unwrap().0, 401);
	fixture.cleanup().await;
}
