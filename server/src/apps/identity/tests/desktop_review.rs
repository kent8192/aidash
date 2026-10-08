use common::upstream_fixtures;
use futures_util::FutureExt;
use http::StatusCode;
use reinhardt::ServerRouter as Router;
use upstream_fixtures::handler;
#[path = "../../execution/tests/support/legacy.rs"]
mod common;

use aidash_server::{config::OidcConfig, federation::Federation};

use base64::{
	Engine,
	engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};

use openidconnect::{
	PrivateSigningKey,
	core::{CoreJsonWebKeySet, CoreRsaPrivateSigningKey},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{
	Arc,
	atomic::{AtomicBool, Ordering},
};

use uuid::Uuid;

const SUBJECT: &str = "desktop-review-user";
const SID: &str = "desktop-review-provider-session";
const ORIGIN: &str = "http://127.0.0.1:8080";
const PEM: &str = include_str!(concat!(
	env!("CARGO_MANIFEST_DIR"),
	"/src/apps/execution/tests/fixtures/oidc/signing-test-only.pem"
));

struct Fixture {
	// Retain disposable infrastructure when an Act replaces the application.
	_owner: common::RuntimeFixture,
	f: Federation,
	url: String,
	schema: String,
	identity: Uuid,
	browser: Uuid,
	outage: Arc<AtomicBool>,
	server: upstream_fixtures::FixedServerGuard,
	app: common::TestApplication,
}
impl Drop for Fixture {
	fn drop(&mut self) {
		self.server.abort();
	}
}
impl Fixture {
	async fn handoff(&self) -> Value {
		// Seed approved consent so the regression controls the exchange's lock order.
		let code = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
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
				Expr::val(Uuid::new_v4()),
				Expr::val(&state),
				Expr::val(URL_SAFE_NO_PAD.encode(Sha256::digest(&verifier))),
				Expr::val(redirect),
				Expr::val(ORIGIN),
				Expr::val(self.browser),
				Expr::val(Sha256::digest(&code).to_vec()),
				Expr::value(chrono::Utc::now() + chrono::Duration::minutes(5)),
			])
			.to_string(PostgresQueryBuilder);
		sqlx::query(&insert)
			.execute(self.f.store.pool.driver())
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
			.and_where(Expr::col(Alias::new("application_name")).eq(Expr::value(&self.schema)))
			.and_where(Expr::col(Alias::new("wait_event_type")).eq(Expr::value("Lock")))
			.and_where(Expr::col(Alias::new("query")).like(format!("%{fragment}%")))
			.to_string(PostgresQueryBuilder);
		tokio::time::timeout(std::time::Duration::from_secs(10), async {
			while sqlx::query_scalar::<_, i64>(&query)
				.fetch_one(self.f.store.pool.driver())
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
	app: &common::TestApplication,
	method: &str,
	path: &str,
	headers: &[(&str, &str)],
	body: bytes::Bytes,
) -> reinhardt::test::TestResponse {
	common::http_response(app, method, path, headers, &body).await
}
async fn request_json(
	app: &common::TestApplication,
	method: &str,
	path: &str,
	input: Value,
) -> (u16, Value) {
	let response = request(
		app,
		method,
		path,
		&[("content-type", "application/json")],
		bytes::Bytes::from(input.to_string()),
	)
	.await;
	let status = response.status().as_u16();
	let bytes = response.body();
	(status, serde_json::from_slice(bytes).unwrap())
}

async fn exchange(app: &common::TestApplication, input: Value) -> (u16, Value) {
	request_json(app, "POST", "/auth/desktop/exchange", input).await
}
async fn logout(app: &common::TestApplication, token: String) -> u16 {
	request(
		app,
		"POST",
		"/auth/backchannel-logout",
		&[("content-type", "application/x-www-form-urlencoded")],
		bytes::Bytes::from(format!("logout_token={token}")),
	)
	.await
	.status()
	.as_u16()
}

#[rstest::rstest]
#[case::exchange_fresh("exchange", false)]
#[case::exchange_provider_revalidation("exchange", true)]
#[case::exchange_remains_single_use("exchange_reuse", false)]
#[case::refresh_fresh("refresh", false)]
#[case::refresh_provider_revalidation("refresh", true)]
#[tokio::test]
async fn desktop_authentication_bursts_complete_with_one_pool_connection(
	#[future(awt)]
	#[from(review_fixture)]
	fixture: Fixture,
	#[case] endpoint: &str,
	#[case] stale_identity: bool,
) {
	let mut fixture = fixture;
	let mut inputs = Vec::new();
	if endpoint == "exchange_reuse" {
		inputs = vec![fixture.handoff().await; 30];
	} else if endpoint == "exchange" {
		for _ in 0..30 {
			inputs.push(fixture.handoff().await);
		}
	} else {
		let (status, tokens) = exchange(&fixture.app.clone(), fixture.handoff().await).await;
		assert_eq!(status, 200, "{tokens}");
		let input = json!({"refresh_token": tokens["refresh_token"], "next_token": format!("aidash_refresh_{}", "n".repeat(64))});
		inputs = vec![input; 30];
	}
	if stale_identity {
		let update = Query::update()
			.table(Alias::new("dashboard_identities"))
			.value_expr(
				Alias::new("last_valid_at"),
				Expr::cust("clock_timestamp()-interval '2 minutes'"),
			)
			.to_string(PostgresQueryBuilder);
		sqlx::query(&update)
			.execute(fixture.f.store.pool.driver())
			.await
			.unwrap();
	}
	// Retain schema initialization but make any nested pool acquisition fail.
	let pool = fixture
		.f
		.store
		.pool
		.options()
		.clone()
		.max_connections(1)
		.min_connections(0)
		.acquire_timeout(std::time::Duration::from_secs(3))
		.connect_with(fixture.f.store.pool.connect_options().as_ref().clone())
		.await
		.unwrap();
	let old_pool = std::mem::replace(&mut fixture.f.store.pool, pool.clone().into());
	fixture.f.registry =
		aidash_server::registry::Registry::new(pool.clone(), &fixture.f.config.node_id).unwrap();
	old_pool.close().await;
	// Act: rebuild the native application after changing the runtime authority.
	fixture.app = common::application_with_settings(
		fixture.f.clone(),
		aidash_server::http::Settings {
			auth_burst: 10000,
			..Default::default()
		},
	)
	.await;
	let app = fixture.app.clone();
	let path = if endpoint == "exchange_reuse" {
		"/auth/desktop/exchange".to_owned()
	} else {
		format!("/auth/desktop/{endpoint}")
	};
	let responses = tokio::time::timeout(
		std::time::Duration::from_secs(20),
		futures_util::future::join_all(
			inputs
				.into_iter()
				.map(|input| request_json(&app, "POST", &path, input)),
		),
	)
	.await
	.expect("authentication must not deadlock on pool capacity");
	if endpoint == "exchange_reuse" {
		assert_eq!(
			responses
				.iter()
				.filter(|(status, _)| *status == 200)
				.count(),
			1
		);
		assert_eq!(
			responses
				.iter()
				.filter(|(status, _)| *status == 401)
				.count(),
			29
		);
	} else {
		for (status, body) in responses {
			assert_eq!(status, 200, "{endpoint}: {body}");
		}
	}
	// Unrelated database work can still borrow the sole connection afterward.
	let probe = Query::select()
		.expr(Expr::val(1))
		.to_string(PostgresQueryBuilder);
	assert_eq!(
		sqlx::query_scalar::<_, i32>(&probe)
			.fetch_one(&pool)
			.await
			.unwrap(),
		1
	);
	fixture.cleanup().await;
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn operator_token_with_desktop_prefix_retains_api_access(
	#[future(awt)]
	#[from(review_fixture)]
	fixture: Fixture,
	#[case] oidc_enabled: bool,
) {
	let mut fixture = fixture;
	fixture.f.config.api_token = format!("aidash_desktop_{}", "operator".repeat(10));
	if !oidc_enabled {
		fixture.f.config.oidc = None;
	}
	// Act: rebuild the native application after changing the runtime authority.
	fixture.app = common::application_with_settings(
		fixture.f.clone(),
		aidash_server::http::Settings {
			auth_burst: 10000,
			..Default::default()
		},
	)
	.await;
	let app = fixture.app.clone();
	let token = format!("Bearer {}", fixture.f.config.api_token);
	let response = request(
		&app,
		"GET",
		"/api/session",
		&[("authorization", &token)],
		bytes::Bytes::new(),
	)
	.await;
	assert_eq!(response.status(), StatusCode::OK);
	let body: Value = serde_json::from_slice(response.body()).unwrap();
	assert_eq!(body["access"]["kind"], "operator", "{body}");
	assert_eq!(
		request(
			&app,
			"GET",
			"/api/session",
			&[("authorization", "Bearer aidash_desktop_unknown")],
			bytes::Bytes::new()
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
	#[from(review_fixture)]
	fixture: Fixture,
	#[case] idle: Option<i64>,
	#[case] revoked: bool,
	#[case] refreshed: bool,
	#[from(refresh_control)] refresh_control: (
		tokio::sync::watch::Sender<bool>,
		tokio::sync::watch::Receiver<bool>,
	),
) {
	let update = Query::update()
		.table(Alias::new("dashboard_sessions"))
		.value_expr(Alias::new("desktop"), idle.is_some())
		.value_expr(Alias::new("desktop_idle_seconds"), Expr::value(idle))
		.value_expr(
			Alias::new("last_activity_at"),
			Expr::cust("clock_timestamp()-interval '1 hour'"),
		)
		.value_expr(
			Alias::new("revoked_at"),
			Expr::val(if revoked {
				Some(chrono::Utc::now())
			} else {
				None
			}),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.execute(fixture.f.store.pool.driver())
		.await
		.unwrap();
	let update = Query::update()
		.table(Alias::new("dashboard_identities"))
		.value_expr(
			Alias::new("last_valid_at"),
			Expr::cust("clock_timestamp()-interval '16 minutes'"),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.execute(fixture.f.store.pool.driver())
		.await
		.unwrap();
	let (stop, stopping) = refresh_control;
	// A queued stop still allows the first refresh pass to finish.
	stop.send(true).unwrap();
	// Act: refresh after applying this case-specific session and identity lifetime.
	aidash_server::dashboard_auth::refresh_active(fixture.f.clone(), stopping)
		.await
		.unwrap();
	let query = Query::select()
		.column(Alias::new("last_valid_at"))
		.from(Alias::new("dashboard_identities"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::value(fixture.identity)))
		.to_string(PostgresQueryBuilder);
	let checked: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(&query)
		.fetch_one(fixture.f.store.pool.driver())
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
	#[from(review_fixture)]
	fixture: Fixture,
	#[from(refresh_control)] refresh_control: (
		tokio::sync::watch::Sender<bool>,
		tokio::sync::watch::Receiver<bool>,
	),
) {
	let app = fixture.app.clone();
	let (status, tokens) = exchange(&app, fixture.handoff().await).await;
	assert_eq!(status, 200, "{tokens}");
	let update = Query::update()
		.table(Alias::new("dashboard_sessions"))
		.value_expr(
			Alias::new("last_activity_at"),
			Expr::cust("clock_timestamp()-interval '1 hour'"),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.execute(fixture.f.store.pool.driver())
		.await
		.unwrap();
	let update = Query::update()
		.table(Alias::new("dashboard_identities"))
		.value_expr(
			Alias::new("last_valid_at"),
			Expr::cust("clock_timestamp()-interval '16 minutes'"),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.execute(fixture.f.store.pool.driver())
		.await
		.unwrap();
	let (stop, stopping) = refresh_control;
	stop.send(true).unwrap();
	// Act: refresh after applying this case-specific session and identity lifetime.
	aidash_server::dashboard_auth::refresh_active(fixture.f.clone(), stopping)
		.await
		.unwrap();
	fixture.outage.store(true, Ordering::SeqCst);
	// Advance past freshness while remaining within the existing outage grace.
	let update = Query::update()
		.table(Alias::new("dashboard_identities"))
		.value_expr(
			Alias::new("last_valid_at"),
			Expr::col(Alias::new("last_valid_at")).sub(Expr::cust("interval '2 minutes'")),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.execute(fixture.f.store.pool.driver())
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
	#[from(review_fixture)]
	fixture: Fixture,
	#[case] selector: &str,
) {
	let input = fixture.handoff().await;
	let app = fixture.app.clone();
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
		.and_where(Expr::col(Alias::new("id")).eq(Expr::value(fixture.browser)))
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
		.from_subquery(source)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&insert)
		.execute(fixture.f.store.pool.driver())
		.await
		.unwrap();
	let mut tx = fixture.f.store.pool.driver().begin().await.unwrap();
	// The exchange obtains identity SHARE before waiting for this browser row.
	let lock = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("dashboard_sessions"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::value(fixture.browser)))
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
		.and_where(Expr::col(Alias::new("id")).eq(Expr::value(other_session)))
		.to_string(PostgresQueryBuilder);
	let revoked: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(&select)
		.fetch_one(fixture.f.store.pool.driver())
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
			bytes::Bytes::new()
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
	#[from(review_fixture)]
	fixture: Fixture,
) {
	let input = fixture.handoff().await;
	let app = fixture.app.clone();
	let mut tx = fixture.f.store.pool.driver().begin().await.unwrap();
	let lock = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("dashboard_sessions"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::value(fixture.browser)))
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

#[rstest::fixture]
fn review_identity() -> Uuid {
	Uuid::new_v4()
}
#[rstest::fixture]
fn review_browser() -> Uuid {
	Uuid::new_v4()
}
#[rstest::fixture]
fn review_outage() -> Arc<AtomicBool> {
	Arc::new(AtomicBool::new(false))
}
#[rstest::fixture]
fn review_router(
	#[from(upstream_fixtures::fixed_listener)] listener: upstream_fixtures::ListenerFuture,
	#[from(review_outage)] outage: Arc<AtomicBool>,
) -> upstream_fixtures::RouterFuture {
	async move {
		let issuer = format!("http://{}/issuer", listener.await.local_addr().unwrap());
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

		let unavailable = outage.clone();
		let provider = Router::new()
			.handler(
				"/issuer/.well-known/openid-configuration",
				handler(http::Method::GET, move |_request: reinhardt::Request| {
					let metadata = metadata.clone();
					async move { reinhardt::Response::ok().with_json(&metadata).unwrap() }
				}),
			)
			.handler(
				"/issuer/jwks",
				handler(http::Method::GET, move |_request: reinhardt::Request| {
					let keys = keys.clone();
					async move { reinhardt::Response::ok().with_json(&keys).unwrap() }
				}),
			)
			.handler(
				"/issuer/protocol/openid-connect/token",
				handler(http::Method::POST, |_request: reinhardt::Request| async {
					reinhardt::Response::ok()
						.with_json(&json!({"access_token": "status-fixture"}))
						.unwrap()
				}),
			)
			.handler(
				"/issuer/admin/users/desktop-review-user",
				handler(http::Method::GET, move |_request: reinhardt::Request| {
					let unavailable = unavailable.clone();
					async move {
						let status = if unavailable.load(Ordering::SeqCst) {
							StatusCode::SERVICE_UNAVAILABLE
						} else {
							StatusCode::OK
						};
						reinhardt::Response::new(status)
							.with_json(&json!({"id": SUBJECT, "enabled": true}))
							.unwrap()
					}
				}),
			);

		Arc::new(provider)
	}
	.boxed()
	.shared()
}
#[rstest::fixture]
fn review_runtime(
	#[from(upstream_fixtures::fixed_listener)] listener: upstream_fixtures::ListenerFuture,
	#[from(review_identity)] identity: Uuid,
	#[from(review_browser)] browser: Uuid,
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) -> common::RuntimeFuture {
	async move {
		let issuer = format!("http://{}/issuer", listener.await.local_addr().unwrap());
		let mut owner = runtime.await;
		let f = &mut owner.federation;
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

		let insert = Query::insert()
			.into_table(Alias::new("dashboard_identities"))
			.columns(["id", "issuer", "subject", "last_valid_at"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::val(identity))
					.expr(Expr::val(issuer))
					.expr(Expr::val(SUBJECT))
					.expr(Expr::cust("clock_timestamp()"))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder);
		sqlx::query(&insert)
			.execute(f.store.pool.driver())
			.await
			.unwrap();
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
			.from_subquery(
				Query::select()
					.expr(Expr::val(browser))
					.expr(Expr::val(identity))
					.expr(Expr::val(Sha256::digest(b"review-browser").to_vec()))
					.expr(Expr::val(Sha256::digest(b"review-csrf").to_vec()))
					.expr(Expr::cust("clock_timestamp()"))
					.expr(Expr::cust("clock_timestamp()"))
					.expr(Expr::cust("clock_timestamp()+interval '12 hours'"))
					.expr(Expr::val(SID))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder);
		sqlx::query(&insert)
			.execute(f.store.pool.driver())
			.await
			.unwrap();

		owner
	}
	.boxed()
	.shared()
}
#[rstest::fixture]
async fn review_fixture(
	#[from(review_ids)] _state: ReviewIds,
	#[from(review_outage)] outage: Arc<AtomicBool>,
	#[from(upstream_fixtures::fixed_listener)] _listener: upstream_fixtures::ListenerFuture,
	#[from(review_router)]
	#[with(_listener.clone(),outage.clone())]
	_router: upstream_fixtures::RouterFuture,
	#[from(review_runtime)]
	#[with(_listener.clone(),_state.identity,_state.browser)]
	_runtime: common::RuntimeFuture,
	#[from(common::direct_application)]
	#[with(aidash_server::http::Settings {auth_burst:10000,..Default::default()},aidash_server::sse::Service::new(Default::default()),Arc::new(|r|r),_runtime.clone())]
	application: common::ApplicationFuture,
	#[future(awt)]
	#[from(upstream_fixtures::fixed_upstream)]
	#[with(None,_listener.clone(),_router.clone())]
	server: upstream_fixtures::FixedServerGuard,
) -> Fixture {
	let ReviewIds { identity, browser } = _state;
	let application = application.await;
	let (f, url, schema) = application.runtime.parts();
	Fixture {
		_owner: application.runtime,
		f,
		url,
		schema,
		identity,
		browser,
		outage,
		server,
		app: application.application,
	}
}

#[rstest::fixture]
fn refresh_control() -> (
	tokio::sync::watch::Sender<bool>,
	tokio::sync::watch::Receiver<bool>,
) {
	tokio::sync::watch::channel(false)
}

#[derive(Clone)]
struct ReviewIds {
	identity: Uuid,
	browser: Uuid,
}
#[rstest::fixture]
fn review_ids(
	#[from(review_identity)] identity: Uuid,
	#[from(review_browser)] browser: Uuid,
) -> ReviewIds {
	ReviewIds { identity, browser }
}
