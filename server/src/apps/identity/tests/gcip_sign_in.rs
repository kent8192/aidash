//! Signed token and fake tenant Admin API acceptance through the production router.
#[path = "../../execution/tests/support/legacy.rs"]
mod common;
use aidash_integrations::gcip::{AccessToken, AccountLookup, Services, SigningKeys, TokenVerifier};
use aidash_server::{config::GcipConfig, federation::Federation};
use async_trait::async_trait;
use chrono::Utc;
use common::{TestEnvironment, test_environment};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, encode};
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::{Value, json};
use std::sync::{
	Arc,
	atomic::{AtomicBool, AtomicI64, Ordering},
};

struct Keys;
#[async_trait]
impl SigningKeys for Keys {
	async fn key(&self, kid: &str) -> aidash_application::Result<DecodingKey> {
		if kid != "fixture" {
			return Err(aidash_application::Error::Unauthorized);
		}
		Ok(DecodingKey::from_rsa_pem(include_bytes!(concat!(
			env!("CARGO_MANIFEST_DIR"),
			"/../crates/aidash-integrations/src/gcip/signing-test-only-public.pem"
		)))
		.unwrap())
	}
}
struct Credentials;
#[async_trait]
impl AccessToken for Credentials {
	async fn token(&self) -> aidash_application::Result<String> {
		Ok("service-token".into())
	}
}
#[derive(Default)]
struct Status {
	disabled: AtomicBool,
	since: AtomicI64,
	outage: AtomicBool,
}
struct FakeAdmin(tokio::task::JoinHandle<()>);
impl Drop for FakeAdmin {
	fn drop(&mut self) {
		self.0.abort();
	}
}
async fn configure(f: &mut Federation) -> (Arc<Status>, FakeAdmin) {
	use axum::{
		Json, Router,
		http::{HeaderMap, StatusCode},
		routing::post,
	};
	let status = Arc::new(Status::default());
	let handler = status.clone();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let router = Router::new().route("/v1/projects/fixture-project/tenants/{pool}/accounts:lookup", post(move |axum::extract::Path(pool): axum::extract::Path<String>, headers: HeaderMap, Json(body): Json<Value>| {
        let status = handler.clone(); async move {
            assert_eq!(headers["authorization"], "Bearer service-token"); assert!(matches!(pool.as_str(), "pool-a" | "pool-b"));
            if status.outage.load(Ordering::SeqCst) { return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({}))); }
            (StatusCode::OK, Json(json!({"users":[{"localId":body["localId"][0],"tenantId":pool,"disabled":status.disabled.load(Ordering::SeqCst),"validSince":status.since.load(Ordering::SeqCst).to_string()}]})))
        }
    }));
	let server = FakeAdmin(tokio::spawn(async move {
		axum::serve(listener, router).await.unwrap();
	}));
	f.config.gcip = Some(GcipConfig {
		project_id: "fixture-project".into(),
		web_api_key: "public-key".into(),
		public_origin: "http://127.0.0.1:8080".into(),
		tenant_bindings: [
			("pool-a".into(), "acme".into()),
			("pool-b".into(), "other".into()),
		]
		.into(),
		providers: Default::default(),
		password_sign_up: Default::default(),
		auth_helper: Default::default(),
		session_absolute_seconds: 43200,
		session_idle_seconds: 1800,
	});
	f.store = f
		.store
		.clone()
		.with_dashboard_policy(f.config.dashboard_policy());
	f.gcip = Some(Arc::new(Services {
		verifier: TokenVerifier {
			project: "fixture-project".into(),
			keys: Arc::new(Keys),
		},
		status: Arc::new(AccountLookup {
			project: "fixture-project".into(),
			endpoint,
			client: reqwest::Client::new(),
			credentials: Arc::new(Credentials),
		}),
	}));
	(status, server)
}
fn browser() -> reqwest::Client {
	reqwest::Client::builder()
		.redirect(reqwest::redirect::Policy::none())
		.build()
		.unwrap()
}
fn token(pool: &str, name: &str, auth_time: i64) -> String {
	token_with_provider(pool, name, auth_time, "password")
}
fn token_with_provider(pool: &str, name: &str, auth_time: i64, provider: &str) -> String {
	let now = Utc::now().timestamp();
	let mut header = Header::new(Algorithm::RS256);
	header.kid = Some("fixture".into());
	encode(&header, &json!({"iss":"https://securetoken.google.com/fixture-project","aud":"fixture-project","sub":"person","iat":now,"exp":now+3600,"auth_time":auth_time,"firebase":{"tenant":pool,"sign_in_provider":provider},"email_verified":true,"email":"person@example.test","name":name,"operator":true}), &EncodingKey::from_rsa_pem(include_bytes!("../../execution/tests/fixtures/oidc/signing-test-only.pem")).unwrap()).unwrap()
}
async fn transaction(app: &common::TestApplication, org: &str) -> (String, String) {
	let response = browser()
		.get(app.url(format!("/auth/login?org={org}&return_to=%2Fsettings")))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 303);
	assert_eq!(response.headers()["cache-control"], "no-store");
	let state = response.headers()["location"]
		.to_str()
		.unwrap()
		.split("state=")
		.nth(1)
		.unwrap()
		.to_owned();
	let cookie = response.headers()["set-cookie"]
		.to_str()
		.unwrap()
		.split(';')
		.next()
		.unwrap()
		.to_owned();
	(state, cookie)
}
async fn exchange(
	app: &common::TestApplication,
	state: &str,
	cookie: &str,
	token: &str,
	origin: &str,
) -> reqwest::Response {
	browser()
		.post(app.url("/auth/gcip/exchange"))
		.header("cookie", cookie)
		.header("origin", origin)
		.json(&json!({"state":state,"id_token":token}))
		.send()
		.await
		.unwrap()
}
async fn sign_in(
	app: &common::TestApplication,
	org: &str,
	pool: &str,
	name: &str,
) -> (String, String) {
	let (state, cookie) = transaction(app, org).await;
	let response = exchange(
		app,
		&state,
		&cookie,
		&token(pool, name, Utc::now().timestamp()),
		"http://127.0.0.1:8080",
	)
	.await;
	let status = response.status();
	let headers = response.headers().clone();
	let body: Value = response.json().await.unwrap();
	assert_eq!(status, 200, "{body}");
	assert_eq!(body, json!({"return_to":"/settings"}));
	assert_eq!(headers["cache-control"], "no-store");
	let cookies: Vec<_> = headers
		.get_all("set-cookie")
		.iter()
		.map(|c| c.to_str().unwrap().to_owned())
		.collect();
	assert!(cookies[0].contains("HttpOnly"));
	let session = cookies[0].split(';').next().unwrap().to_owned();
	let csrf = cookies[1]
		.split(';')
		.next()
		.unwrap()
		.split('=')
		.nth(1)
		.unwrap()
		.to_owned();
	(session, csrf)
}

#[rstest::rstest]
#[tokio::test]
async fn provider_allowlist_is_enforced_for_an_existing_uid_before_admin_io(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (mut f, url, schema) = common::setup(&environment).await;
	let (status, _admin) = configure(&mut f).await;
	let app = common::application(f.clone()).await;
	sign_in(&app, "acme", "pool-a", "Person").await;
	f.config.gcip.as_mut().unwrap().providers.insert(
		"pool-a".into(),
		vec!["oidc.company".into(), "saml.company".into()],
	);
	f.store = f
		.store
		.clone()
		.with_dashboard_policy(f.config.dashboard_policy());
	let changed = common::application(f.clone()).await;
	status.outage.store(true, Ordering::SeqCst);
	for provider in ["password", "google.com", "custom"] {
		let (state, cookie) = transaction(&changed, "acme").await;
		let configuration: Value = browser()
			.get(changed.url(format!("/auth/gcip/transaction?state={state}")))
			.header("cookie", &cookie)
			.send()
			.await
			.unwrap()
			.json()
			.await
			.unwrap();
		assert_eq!(
			configuration["providers"],
			json!(["oidc.company", "saml.company"])
		);
		assert_eq!(
			exchange(
				&changed,
				&state,
				&cookie,
				&token_with_provider("pool-a", "Person", Utc::now().timestamp(), provider),
				"http://127.0.0.1:8080"
			)
			.await
			.status(),
			403,
			"{provider}"
		);
	}
	status.outage.store(false, Ordering::SeqCst);
	for provider in ["oidc.company", "saml.company"] {
		let (state, cookie) = transaction(&changed, "acme").await;
		assert_eq!(
			exchange(
				&changed,
				&state,
				&cookie,
				&token_with_provider("pool-a", "Person", Utc::now().timestamp(), provider),
				"http://127.0.0.1:8080"
			)
			.await
			.status(),
			200,
			"{provider}"
		);
	}
	// An explicitly empty list disables every method rather than selecting defaults.
	f.config
		.gcip
		.as_mut()
		.unwrap()
		.providers
		.insert("pool-a".into(), vec![]);
	f.store = f
		.store
		.clone()
		.with_dashboard_policy(f.config.dashboard_policy());
	let disabled = common::application(f.clone()).await;
	let (state, cookie) = transaction(&disabled, "acme").await;
	assert_eq!(
		exchange(
			&disabled,
			&state,
			&cookie,
			&token_with_provider("pool-a", "Person", Utc::now().timestamp(), "saml.company"),
			"http://127.0.0.1:8080"
		)
		.await
		.status(),
		403
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn browser_bound_exchange_rejects_bad_origin_browser_pool_auth_time_expiry_and_replay(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (mut f, url, schema) = common::setup(&environment).await;
	let (_status, _admin) = configure(&mut f).await;
	let app = common::application(f.clone()).await;
	let config: Value = browser()
		.get(app.url("/auth/config"))
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(config["provider"], "gcip");
	let organization = browser().get(app.url("/auth/login")).send().await.unwrap();
	assert!(
		organization.headers()["location"]
			.to_str()
			.unwrap()
			.starts_with("/sign-in?")
	);
	let (state, cookie) = transaction(&app, "acme").await;
	let valid = token("pool-a", "Person", Utc::now().timestamp());
	for (browser_cookie, jwt, origin, expected) in [
		(
			cookie.as_str(),
			valid.as_str(),
			"https://attacker.example",
			403,
		),
		(
			"aidash-login=wrong",
			valid.as_str(),
			"http://127.0.0.1:8080",
			401,
		),
		(
			cookie.as_str(),
			"invalid-token",
			"http://127.0.0.1:8080",
			401,
		),
	] {
		let response = exchange(&app, &state, browser_cookie, jwt, origin).await;
		assert_eq!(response.status(), expected);
		assert_eq!(response.headers()["cache-control"], "no-store");
	}
	for invalid in [
		token("pool-b", "Person", Utc::now().timestamp()),
		token("pool-a", "Person", 1),
	] {
		assert_eq!(
			exchange(&app, &state, &cookie, &invalid, "http://127.0.0.1:8080")
				.await
				.status(),
			401
		);
	}
	let missing_origin = browser()
		.post(app.url("/auth/gcip/exchange"))
		.header("cookie", &cookie)
		.json(&json!({"state":state,"id_token":valid}))
		.send()
		.await
		.unwrap();
	assert_eq!(missing_origin.status(), 403);
	assert_eq!(
		exchange(
			&app,
			"missing-transaction",
			&cookie,
			&valid,
			"http://127.0.0.1:8080"
		)
		.await
		.status(),
		401
	);
	assert_eq!(
		exchange(&app, &state, &cookie, &valid, "http://127.0.0.1:8080")
			.await
			.status(),
		200
	);
	assert_eq!(
		exchange(&app, &state, &cookie, &valid, "http://127.0.0.1:8080")
			.await
			.status(),
		401
	);
	let (state, cookie) = transaction(&app, "acme").await;
	let expire = Query::update()
		.table(Alias::new("dashboard_login_transactions"))
		.value_expr(
			Alias::new("expires_at"),
			Expr::cust("clock_timestamp()-interval '1 second'"),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&expire)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
	assert_eq!(
		exchange(&app, &state, &cookie, &valid, "http://127.0.0.1:8080")
			.await
			.status(),
		401
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn tenant_identity_keys_approval_display_and_status_revocation_are_distinct(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (mut f, url, schema) = common::setup(&environment).await;
	let (status, _admin) = configure(&mut f).await;
	let app = common::application(f.clone()).await;
	let (cookie, csrf) = sign_in(&app, "acme", "pool-a", "First").await;
	let registrations = browser()
		.post(app.url("/auth/registration"))
		.header("cookie", &cookie)
		.header("origin", "http://127.0.0.1:8080")
		.header("x-aidash-csrf", &csrf)
		.send()
		.await
		.unwrap();
	assert_eq!(registrations.status(), 200);
	let registration: Value = registrations.json().await.unwrap();
	let path = format!(
		"/api/dashboard/registrations/{}/approve",
		registration["id"].as_str().unwrap()
	);
	let rejected = browser()
		.post(app.url(&path))
		.bearer_auth(&f.config.api_token)
		.json(&json!({"tenant":"other","subject":"alice"}))
		.send()
		.await
		.unwrap();
	assert_eq!(rejected.status(), 403);
	let _ = sign_in(&app, "other", "pool-b", "Other").await;
	let identities: Value = browser()
		.get(app.url("/api/dashboard/identities"))
		.bearer_auth(&f.config.api_token)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(identities.as_array().unwrap().len(), 2);
	assert!(
		identities
			.as_array()
			.unwrap()
			.iter()
			.all(|i| i["subject"] == "person")
	);
	let _ = sign_in(&app, "acme", "pool-a", "Changed").await;
	let identity_id = registration["identity_id"].as_str().unwrap();
	let identity_path = format!("/api/dashboard/identities/{identity_id}");
	let get_identity = || {
		browser()
			.get(app.url(&identity_path))
			.bearer_auth(&f.config.api_token)
	};
	let view: Value = get_identity().send().await.unwrap().json().await.unwrap();
	assert_eq!(view["display_name"], "Changed");
	assert_eq!(view["verified_email"], "person@example.test");
	let reject_path = format!(
		"/api/dashboard/registrations/{}/reject",
		registration["id"].as_str().unwrap()
	);
	assert_eq!(
		browser()
			.post(app.url(reject_path))
			.bearer_auth(&f.config.api_token)
			.send()
			.await
			.unwrap()
			.status(),
		200
	);
	let view: Value = get_identity().send().await.unwrap().json().await.unwrap();
	assert!(view["display_name"].is_null() && view["verified_email"].is_null());
	let force_stale = Query::update()
		.table(Alias::new("dashboard_identities"))
		.value_expr(
			Alias::new("last_valid_at"),
			Expr::cust("clock_timestamp()-interval '6 minutes'"),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&force_stale)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
	status.outage.store(true, Ordering::SeqCst);
	assert_eq!(
		browser()
			.get(app.url("/auth/session"))
			.header("cookie", &cookie)
			.send()
			.await
			.unwrap()
			.status(),
		200
	);
	let deadline = Query::update()
		.table(Alias::new("dashboard_identities"))
		.value_expr(
			Alias::new("last_valid_at"),
			Expr::cust("clock_timestamp()-interval '16 minutes'"),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&deadline)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
	assert_eq!(
		browser()
			.get(app.url("/auth/session"))
			.header("cookie", &cookie)
			.send()
			.await
			.unwrap()
			.status(),
		503
	);
	status.outage.store(false, Ordering::SeqCst);
	status
		.since
		.store(Utc::now().timestamp() + 1, Ordering::SeqCst);
	let revoked = browser()
		.get(app.url("/auth/session"))
		.header("cookie", &cookie)
		.send()
		.await
		.unwrap();
	assert_eq!(revoked.status(), 401);
	let view: Value = get_identity().send().await.unwrap().json().await.unwrap();
	assert!(
		view["disabled_at"].is_null(),
		"validSince cannot disable the Identity"
	);
	status.since.store(0, Ordering::SeqCst);
	status.disabled.store(true, Ordering::SeqCst);
	let (state, bound) = transaction(&app, "acme").await;
	assert_eq!(
		exchange(
			&app,
			&state,
			&bound,
			&token("pool-a", "Denied", Utc::now().timestamp()),
			"http://127.0.0.1:8080"
		)
		.await
		.status(),
		403
	);
	let view: Value = get_identity().send().await.unwrap().json().await.unwrap();
	assert!(
		!view["disabled_at"].is_null(),
		"disabled sign-in must disable an existing Identity"
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn desktop_gcip_handoff_inherits_browser_auth_time_and_revokes_both_sessions(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
	use sha2::{Digest, Sha256};
	let (mut f, url, schema) = common::setup(&environment).await;
	let (status, _admin) = configure(&mut f).await;
	let app = common::application(f.clone()).await;
	let (cookie, csrf) = sign_in(&app, "acme", "pool-a", "Person").await;
	let cookie = format!("{cookie}; aidash-csrf={csrf}");
	let verifier = "a".repeat(64);
	let state = "b".repeat(64);
	let redirect = "http://127.0.0.1:43217/callback";
	let start:Value=browser().post(app.url("/auth/desktop/start")).json(&json!({"redirect_uri":redirect,"state":state,"code_challenge":URL_SAFE_NO_PAD.encode(Sha256::digest(&verifier))})).send().await.unwrap().json().await.unwrap();
	let authorization = start["authorization_url"]
		.as_str()
		.unwrap()
		.strip_prefix("http://127.0.0.1:8080")
		.unwrap();
	let consent = browser()
		.get(app.url(authorization))
		.header("cookie", &cookie)
		.send()
		.await
		.unwrap();
	assert_eq!(consent.status(), 200);
	let request = authorization.split("request=").nth(1).unwrap();
	let approve = browser()
		.post(app.url("/auth/desktop/authorize"))
		.header("cookie", &cookie)
		.header("origin", "http://127.0.0.1:8080")
		.form(&[("request", request), ("csrf", csrf.as_str())])
		.send()
		.await
		.unwrap();
	assert_eq!(approve.status(), 303);
	let callback = reqwest::Url::parse(approve.headers()["location"].to_str().unwrap()).unwrap();
	let pairs: std::collections::HashMap<_, _> = callback.query_pairs().collect();
	assert_eq!(pairs["state"], state);
	let desktop = browser()
		.post(app.url("/auth/desktop/exchange"))
		.json(
			&json!({"code":pairs["code"],"state":state,"verifier":verifier,"redirect_uri":redirect}),
		)
		.send()
		.await
		.unwrap();
	let code = desktop.status();
	let tokens: Value = desktop.json().await.unwrap();
	assert_eq!(code, 200, "{tokens}");
	let query = Query::select()
		.columns(["auth_time", "desktop"].map(Alias::new))
		.from(Alias::new("dashboard_sessions"))
		.to_string(PostgresQueryBuilder);
	let sessions: Vec<(Option<chrono::DateTime<Utc>>, bool)> = sqlx::query_as(&query)
		.fetch_all(f.store.pool.driver())
		.await
		.unwrap();
	assert_eq!(sessions.len(), 2);
	assert_eq!(sessions[0].0, sessions[1].0);
	assert!(sessions[0].0.is_some());
	status
		.since
		.store(Utc::now().timestamp() + 1, Ordering::SeqCst);
	let stale = Query::update()
		.table(Alias::new("dashboard_identities"))
		.value_expr(
			Alias::new("last_valid_at"),
			Expr::cust("clock_timestamp()-interval '6 minutes'"),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&stale)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
	assert_eq!(
		browser()
			.get(app.url("/auth/session"))
			.bearer_auth(tokens["access_token"].as_str().unwrap())
			.send()
			.await
			.unwrap()
			.status(),
		401
	);
	assert_eq!(
		browser()
			.get(app.url("/auth/session"))
			.header("cookie", &cookie)
			.send()
			.await
			.unwrap()
			.status(),
		401
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn removing_binding_disables_a_fresh_identity_at_its_next_boundary(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (mut f, url, schema) = common::setup(&environment).await;
	let (_status, _admin) = configure(&mut f).await;
	let app = common::application(f.clone()).await;
	let (cookie, _) = sign_in(&app, "acme", "pool-a", "Person").await;
	f.config
		.gcip
		.as_mut()
		.unwrap()
		.tenant_bindings
		.remove("pool-a");
	f.store = f
		.store
		.clone()
		.with_dashboard_policy(f.config.dashboard_policy());
	let changed = common::application(f.clone()).await;
	assert_eq!(
		browser()
			.get(changed.url("/auth/session"))
			.header("cookie", cookie)
			.send()
			.await
			.unwrap()
			.status(),
		403
	);
	let identities: Value = browser()
		.get(changed.url("/api/dashboard/identities"))
		.bearer_auth(&f.config.api_token)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert!(!identities[0]["disabled_at"].is_null());
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[case::login("login")]
#[case::refresh("refresh")]
#[case::install("install")]
#[tokio::test]
async fn removed_binding_login_disables_an_inactive_identity_and_its_existing_authority(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
	#[case] boundary: &str,
) {
	let (mut f, url, schema) = common::setup(&environment).await;
	let (status, _admin) = configure(&mut f).await;
	let policy = serde_json::from_value(
		json!({"tenant":"acme","subjects":{"alice":{"kind":"user"}},"policies":[]}),
	)
	.unwrap();
	aidash_server::authorization::Authorization {
		pool: f.store.pool.clone(),
	}
	.replace("acme", 0, policy, "operator")
	.await
	.unwrap();
	let app = common::application(f.clone()).await;
	let (cookie, csrf) = sign_in(&app, "acme", "pool-a", "Person").await;
	let registration: Value = browser()
		.post(app.url("/auth/registration"))
		.header("cookie", cookie)
		.header("origin", "http://127.0.0.1:8080")
		.header("x-aidash-csrf", csrf)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	let identity = uuid::Uuid::parse_str(registration["identity_id"].as_str().unwrap()).unwrap();
	let approved = browser()
		.post(app.url(format!(
			"/api/dashboard/registrations/{}/approve",
			registration["id"].as_str().unwrap()
		)))
		.bearer_auth(&f.config.api_token)
		.json(&json!({"tenant":"acme","subject":"alice"}))
		.send()
		.await
		.unwrap();
	assert_eq!(approved.status(), 200);
	let grant = browser()
		.post(app.url(format!(
			"/api/dashboard/identities/{identity}/operator-grant"
		)))
		.bearer_auth(&f.config.api_token)
		.json(&json!({"enabled":true,"expected_revision":0}))
		.send()
		.await
		.unwrap();
	assert_eq!(grant.status(), 200);
	// No live browser session or run remains for background active refresh.
	let expire = Query::update()
		.table(Alias::new("dashboard_sessions"))
		.value_expr(
			Alias::new("expires_at"),
			Expr::val(Utc::now() - chrono::Duration::seconds(1)),
		)
		.and_where(Expr::col("identity_id").eq(Expr::val(identity)))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&expire)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
	let (state, cookie) = transaction(&app, "acme").await;
	f.config
		.gcip
		.as_mut()
		.unwrap()
		.tenant_bindings
		.remove("pool-a");
	f.store = f
		.store
		.clone()
		.with_dashboard_policy(f.config.dashboard_policy());
	let changed = common::application(f.clone()).await;
	let disabled = Query::select()
		.column(Alias::new("disabled_at"))
		.from(Alias::new("dashboard_identities"))
		.and_where(Expr::col("id").eq(Expr::val(identity)))
		.to_string(PostgresQueryBuilder);
	// An invalid token must never be able to retire another user's authority.
	assert_eq!(
		exchange(
			&changed,
			&state,
			&cookie,
			"invalid-token",
			"http://127.0.0.1:8080"
		)
		.await
		.status(),
		401
	);
	assert!(
		sqlx::query_scalar::<_, Option<chrono::DateTime<Utc>>>(&disabled)
			.fetch_one(f.store.pool.driver())
			.await
			.unwrap()
			.is_none()
	);
	// A removed Binding must not contact the retired pool's Admin API.
	status.outage.store(true, Ordering::SeqCst);
	if boundary == "login" {
		assert_eq!(
			exchange(
				&changed,
				&state,
				&cookie,
				&token("pool-a", "Person", Utc::now().timestamp()),
				"http://127.0.0.1:8080"
			)
			.await
			.status(),
			403
		);
	} else if boundary == "install" {
		let mut settings = common::settings_for(&url);
		settings.node.node_id = f.config.node_id.clone();
		settings.node.endpoint = f.config.endpoint.clone();
		settings.node.api_token = f.config.api_token.clone();
		settings.dashboard.gcip = f.config.gcip.clone();
		let context = reinhardt::test::fixtures::injection_context::default();
		let installed =
			aidash_server::bootstrap::initialize(&context, &settings, f.store.pool.connection())
				.await
				.unwrap();
		installed.store.control_pool.close().await;
	} else {
		let (stop, stopping) = tokio::sync::watch::channel(false);
		let runtime = f.clone();
		let mut refresh = FakeAdmin(tokio::spawn(async move {
			aidash_server::dashboard_auth::refresh_active(runtime, stopping)
				.await
				.unwrap();
		}));
		tokio::time::timeout(std::time::Duration::from_secs(5), async {
			loop {
				if sqlx::query_scalar::<_, Option<chrono::DateTime<Utc>>>(&disabled)
					.fetch_one(f.store.pool.driver())
					.await
					.unwrap()
					.is_some()
				{
					break;
				}
				tokio::time::sleep(std::time::Duration::from_millis(20)).await;
			}
		})
		.await
		.unwrap();
		stop.send(true).unwrap();
		(&mut refresh.0).await.unwrap();
	}
	assert!(
		sqlx::query_scalar::<_, Option<chrono::DateTime<Utc>>>(&disabled)
			.fetch_one(f.store.pool.driver())
			.await
			.unwrap()
			.is_some()
	);
	for table in ["dashboard_mappings", "dashboard_operator_grants"] {
		let enabled = Query::select()
			.column(Alias::new("enabled"))
			.from(Alias::new(table))
			.and_where(Expr::col("identity_id").eq(Expr::val(identity)))
			.to_string(PostgresQueryBuilder);
		// Disablement preserves operator-managed approvals, but gates all of
		// them behind an explicit Identity restore rather than a fresh login.
		assert!(
			sqlx::query_scalar::<_, bool>(&enabled)
				.fetch_one(f.store.pool.driver())
				.await
				.unwrap(),
			"{table} approval must still exist while the Identity is disabled"
		);
	}
	f.config
		.gcip
		.as_mut()
		.unwrap()
		.tenant_bindings
		.insert("pool-a".into(), "acme".into());
	f.store = f
		.store
		.clone()
		.with_dashboard_policy(f.config.dashboard_policy());
	status.outage.store(false, Ordering::SeqCst);
	let restored_binding = common::application(f.clone()).await;
	let (state, cookie) = transaction(&restored_binding, "acme").await;
	assert_eq!(
		exchange(
			&restored_binding,
			&state,
			&cookie,
			&token("pool-a", "Person", Utc::now().timestamp()),
			"http://127.0.0.1:8080"
		)
		.await
		.status(),
		403
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn approved_mapping_cannot_cross_the_bound_tenant_at_a_request_boundary(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (mut f, url, schema) = common::setup(&environment).await;
	let (_status, _admin) = configure(&mut f).await;
	for tenant in ["acme", "other"] {
		let policy = serde_json::from_value(
			json!({"tenant":tenant,"subjects":{"alice":{"kind":"user"}},"policies":[]}),
		)
		.unwrap();
		aidash_server::authorization::Authorization {
			pool: f.store.pool.clone(),
		}
		.replace(tenant, 0, policy, "operator")
		.await
		.unwrap();
	}
	let app = common::application(f.clone()).await;
	let (cookie, csrf) = sign_in(&app, "acme", "pool-a", "Person").await;
	let registration: Value = browser()
		.post(app.url("/auth/registration"))
		.header("cookie", &cookie)
		.header("origin", "http://127.0.0.1:8080")
		.header("x-aidash-csrf", csrf)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	let path = format!(
		"/api/dashboard/registrations/{}/approve",
		registration["id"].as_str().unwrap()
	);
	let approved = browser()
		.post(app.url(path))
		.bearer_auth(&f.config.api_token)
		.json(&json!({"tenant":"acme","subject":"alice"}))
		.send()
		.await
		.unwrap();
	let code = approved.status();
	let mapping: Value = approved.json().await.unwrap();
	assert_eq!(code, 200, "{mapping}");
	let selector = format!("mapping:{}", mapping["id"].as_str().unwrap());
	assert_eq!(
		browser()
			.get(app.url("/api/session"))
			.header("cookie", &cookie)
			.header("x-aidash-context", &selector)
			.send()
			.await
			.unwrap()
			.status(),
		200
	);
	let id = uuid::Uuid::parse_str(mapping["id"].as_str().unwrap()).unwrap();
	// A pending retry cannot block an admitted boundary that already holds
	// this Mapping while it obtains the External Identity lock.
	let pending = Query::update()
		.table(Alias::new("dashboard_registration_requests"))
		.value_expr(Alias::new("status"), Expr::val("pending"))
		.and_where(Expr::col("id").eq(Expr::val(
			uuid::Uuid::parse_str(registration["id"].as_str().unwrap()).unwrap(),
		)))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&pending)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
	let mut boundary = f.store.pool.driver().begin().await.unwrap();
	let locked = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("dashboard_mappings"))
		.and_where(Expr::col("id").eq(Expr::val(id)))
		.lock(reinhardt::query::LockType::Share)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&locked)
		.fetch_one(&mut *boundary)
		.await
		.unwrap();
	let retry = browser()
		.post(app.url(format!(
			"/api/dashboard/registrations/{}/approve",
			registration["id"].as_str().unwrap()
		)))
		.bearer_auth(&f.config.api_token)
		.json(&json!({"tenant":"acme","subject":"alice"}))
		.send();
	assert_eq!(
		tokio::time::timeout(std::time::Duration::from_secs(5), retry)
			.await
			.expect("approval must not wait on an enabled Mapping boundary")
			.unwrap()
			.status(),
		409
	);
	boundary.rollback().await.unwrap();
	let credential_query = Query::select()
		.column(Alias::new("credential_id"))
		.from(Alias::new("dashboard_mappings"))
		.and_where(Expr::col("id").eq(Expr::val(id)))
		.to_string(PostgresQueryBuilder);
	let credential: uuid::Uuid = sqlx::query_scalar(&credential_query)
		.fetch_one(f.store.pool.driver())
		.await
		.unwrap();
	for (table, id) in [
		("dashboard_mappings", id),
		("authorization_credentials", credential),
	] {
		let update = Query::update()
			.table(Alias::new(table))
			.value_expr(Alias::new("tenant"), Expr::val("other"))
			.and_where(Expr::col("id").eq(Expr::val(id)))
			.to_string(PostgresQueryBuilder);
		sqlx::query(&update)
			.execute(f.store.pool.driver())
			.await
			.unwrap();
	}
	assert_eq!(
		browser()
			.get(app.url("/api/session"))
			.header("cookie", &cookie)
			.header("x-aidash-context", selector)
			.send()
			.await
			.unwrap()
			.status(),
		403
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn replacement_registration_keeps_freshly_authenticated_display_attributes(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (mut f, url, schema) = common::setup(&environment).await;
	let (_status, _admin) = configure(&mut f).await;
	let app = common::application(f.clone()).await;
	let (cookie, csrf) = sign_in(&app, "acme", "pool-a", "Original").await;
	let previous: Value = browser()
		.post(app.url("/auth/registration"))
		.header("cookie", cookie)
		.header("origin", "http://127.0.0.1:8080")
		.header("x-aidash-csrf", csrf)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	let expire = Query::update()
		.table(Alias::new("dashboard_registration_requests"))
		.value_expr(
			Alias::new("expires_at"),
			Expr::val(Utc::now() - chrono::Duration::seconds(1)),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&expire)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
	let (cookie, csrf) = sign_in(&app, "acme", "pool-a", "Freshly authenticated").await;
	// The UI reads status before showing the replacement submission action.
	let previous_status: Value = browser()
		.get(app.url("/auth/registration"))
		.header("cookie", &cookie)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(previous_status["status"], "expired");
	let replacement = browser()
		.post(app.url("/auth/registration"))
		.header("cookie", &cookie)
		.header("origin", "http://127.0.0.1:8080")
		.header("x-aidash-csrf", csrf)
		.send()
		.await
		.unwrap();
	assert_eq!(replacement.status(), 200);
	let replacement: Value = replacement.json().await.unwrap();
	assert_eq!(replacement["status"], "pending");
	assert_ne!(replacement["id"], previous["id"]);
	assert_eq!(replacement["identity_id"], previous["identity_id"]);
	let path = format!(
		"/api/dashboard/identities/{}",
		replacement["identity_id"].as_str().unwrap()
	);
	let view: Value = browser()
		.get(app.url(&path))
		.bearer_auth(&f.config.api_token)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(view["display_name"], "Freshly authenticated");
	assert_eq!(view["verified_email"], "person@example.test");
	// Expiry without another replacement still erases unmapped display data.
	sqlx::query(&expire)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
	let expired: Value = browser()
		.get(app.url("/auth/registration"))
		.header("cookie", cookie)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(expired["status"], "expired");
	let view: Value = browser()
		.get(app.url(path))
		.bearer_auth(&f.config.api_token)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert!(view["display_name"].is_null() && view["verified_email"].is_null());
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn expired_registration_clears_unmapped_display_attributes(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (mut f, url, schema) = common::setup(&environment).await;
	let (_status, _admin) = configure(&mut f).await;
	let app = common::application(f.clone()).await;
	let (cookie, csrf) = sign_in(&app, "acme", "pool-a", "Person").await;
	let registration: Value = browser()
		.post(app.url("/auth/registration"))
		.header("cookie", &cookie)
		.header("origin", "http://127.0.0.1:8080")
		.header("x-aidash-csrf", csrf)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	let expire = Query::update()
		.table(Alias::new("dashboard_registration_requests"))
		.value_expr(
			Alias::new("expires_at"),
			Expr::cust("clock_timestamp()-interval '1 second'"),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&expire)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
	let expired: Value = browser()
		.get(app.url("/auth/registration"))
		.header("cookie", cookie)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(expired["status"], "expired");
	let path = format!(
		"/api/dashboard/identities/{}",
		registration["identity_id"].as_str().unwrap()
	);
	let view: Value = browser()
		.get(app.url(path))
		.bearer_auth(&f.config.api_token)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert!(view["display_name"].is_null() && view["verified_email"].is_null());
	common::cleanup(f, &url, &schema).await;
}
