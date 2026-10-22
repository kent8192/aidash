//! Signed token and fake tenant Admin API acceptance through the production router.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../../server/src/apps/execution/tests/support/legacy.rs"]
mod common;
use aidash_enterprise_server::gcip::{Gcip, GcipConfig};
use aidash_gcp::gcip::{AccessToken, AccountLookup, Services, TokenVerifier};
use aidash_integrations::jwks::SigningKeys;
use aidash_server::federation::Federation;
use async_trait::async_trait;
use chrono::Utc;
use common::upstream_fixtures;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, encode};
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::{Value, json};
use std::cell::RefCell;
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
			"/../crates/aidash-gcp/src/gcip/signing-test-only-public.pem"
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
/// Abort a background Act when its owner unwinds.
struct AbortOnDrop(tokio::task::JoinHandle<()>);
impl Drop for AbortOnDrop {
	fn drop(&mut self) {
		self.0.abort();
	}
}
#[rstest::fixture]
fn admin_status() -> Arc<Status> {
	Arc::new(Status::default())
}
/// Fake tenant Admin API accounts:lookup contract served by a native catch-all route.
#[rstest::fixture]
fn admin_router(admin_status: Arc<Status>) -> Arc<reinhardt::ServerRouter> {
	let reply = Arc::new(upstream_fixtures::any_handler(
		move |request: reinhardt::Request| {
			let status = admin_status.clone();
			async move {
				assert_eq!(request.method, http::Method::POST);
				let pool = request
					.uri
					.path()
					.strip_prefix("/v1/projects/fixture-project/tenants/")
					.and_then(|path| path.strip_suffix("/accounts:lookup"))
					.expect("tenant accounts:lookup path")
					.to_owned();
				assert_eq!(request.headers["authorization"], "Bearer service-token");
				assert!(matches!(pool.as_str(), "pool-a" | "pool-b" | "pool-s"));
				if status.outage.load(Ordering::SeqCst) {
					return reinhardt::Response::new(http::StatusCode::SERVICE_UNAVAILABLE)
						.with_json(&json!({}))
						.unwrap();
				}
				let body = request.json::<Value>().unwrap();
				reinhardt::Response::new(http::StatusCode::OK)
					.with_json(&json!({"users":[{"localId":body["localId"][0],"tenantId":pool,"disabled":status.disabled.load(Ordering::SeqCst),"validSince":status.since.load(Ordering::SeqCst).to_string()}]}))
					.unwrap()
			}
		},
	));
	Arc::new(
		reinhardt::ServerRouter::new()
			.handler_arc("/", reply.clone())
			.handler_arc("/{*rest}", reply),
	)
}
struct GcipFixture {
	runtime: common::RuntimeFixture,
	status: Arc<Status>,
	_admin: reinhardt::test::fixtures::server::TestServerGuard,
}
/// Production runtime configured for two tenant-bound pools and the fake Admin API.
#[rstest::fixture]
async fn gcip_runtime(
	#[from(admin_status)] status: Arc<Status>,
	#[from(admin_router)]
	#[with(status.clone())]
	_router: Arc<reinhardt::ServerRouter>,
	#[future(awt)]
	#[from(upstream_fixtures::upstream)]
	#[with(_router.clone())]
	admin: reinhardt::test::fixtures::server::TestServerGuard,
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) -> GcipFixture {
	// The fixture database carries the Enterprise migrations too.
	aidash_server::bootstrap::build::install(&aidash_enterprise_server::BUILD);
	let mut runtime = runtime.await;
	configure(&mut runtime.federation, &admin.url);
	// A Tenant Binding addresses requests to its Tenant, which must exist.
	for tenant in ["acme", "other"] {
		common::tenant_root(&runtime.federation, tenant).await;
	}
	GcipFixture {
		runtime,
		status,
		_admin: admin,
	}
}
thread_local! {
	/// The GCIP settings and fake Admin API of this test's runtime. A test
	/// runs on one thread, so a test that changes them reapplies its own.
	static GCIP: RefCell<Option<(GcipConfig, String)>> = const { RefCell::new(None) };
}
fn configure(f: &mut Federation, endpoint: &str) {
	aidash_server::bootstrap::build::install(&aidash_enterprise_server::BUILD);
	let config: GcipConfig = serde_json::from_value(json!({
		"project_id": "fixture-project",
		"web_api_key": "public-key",
		"public_origin": "https://127.0.0.1:8080",
		"tenant_bindings": {"pool-a": "acme", "pool-b": "other"},
		"mfa_mandatory": ["pool-b"],
		"sign_in_domains": {"acme.example": "pool-a", "xn--bcher-kva.example": "pool-b"},
	}))
	.unwrap();
	GCIP.with(|gcip| *gcip.borrow_mut() = Some((config, endpoint.to_owned())));
	apply(f);
}
/// Change this test's GCIP settings and rebuild its sign-in and policy.
fn reconfigure(f: &mut Federation, change: impl FnOnce(&mut GcipConfig)) {
	GCIP.with(|gcip| change(&mut gcip.borrow_mut().as_mut().unwrap().0));
	apply(f);
}
fn gcip_settings() -> GcipConfig {
	GCIP.with(|gcip| gcip.borrow().as_ref().unwrap().0.clone())
}
fn apply(f: &mut Federation) {
	let (config, endpoint) = GCIP.with(|gcip| gcip.borrow().clone().unwrap());
	let services = Services {
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
	};
	f.config.sign_in = Some(Arc::new(Gcip::new(&config, services).unwrap()));
	f.store = f
		.store
		.clone()
		.with_dashboard_policy(f.config.dashboard_policy());
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
	signed_token(pool, "person", name, auth_time, provider)
}
fn signed_token(pool: &str, uid: &str, name: &str, auth_time: i64, provider: &str) -> String {
	let now = Utc::now().timestamp();
	let mut header = Header::new(Algorithm::RS256);
	header.kid = Some("fixture".into());
	encode(&header, &json!({"iss":"https://securetoken.google.com/fixture-project","aud":"fixture-project","sub":uid,"iat":now,"exp":now+3600,"auth_time":auth_time,"firebase":{"tenant":pool,"sign_in_provider":provider},"email_verified":true,"email":format!("{uid}@example.test"),"name":name,"deployment_administrator":true}), &EncodingKey::from_rsa_pem(include_bytes!("../../crates/aidash-gcp/src/gcip/signing-test-only.pem")).unwrap()).unwrap()
}
/// A token as GCIP issues it after a TOTP challenge: the first factor's provider
/// plus `sign_in_second_factor`. Aidash never reads these claims as authority.
fn second_factor_token(pool: &str, uid: &str, name: &str, auth_time: i64) -> String {
	let now = Utc::now().timestamp();
	let mut header = Header::new(Algorithm::RS256);
	header.kid = Some("fixture".into());
	encode(&header, &json!({"iss":"https://securetoken.google.com/fixture-project","aud":"fixture-project","sub":uid,"iat":now,"exp":now+3600,"auth_time":auth_time,"firebase":{"tenant":pool,"sign_in_provider":"password","sign_in_second_factor":"totp","second_factor_identifier":"enrollment-id"},"email_verified":true,"email":format!("{uid}@example.test"),"name":name,"deployment_administrator":true}), &EncodingKey::from_rsa_pem(include_bytes!("../../crates/aidash-gcp/src/gcip/signing-test-only.pem")).unwrap()).unwrap()
}
/// Begin a sign-in through `pool` with an address in a Sign-in Domain routed
/// to it, or in no Sign-in Domain for the Shared Pool.
async fn transaction(app: &common::TestApplication, pool: &str) -> (String, String) {
	let domain = gcip_settings()
		.sign_in_domains
		.into_iter()
		.find_map(|(domain, routed)| (routed == pool).then_some(domain))
		.unwrap_or_else(|| "unmapped.example".into());
	let response = discover(
		app,
		&format!("email=person%40{domain}&return_to=%2Fsettings"),
		Some("https://127.0.0.1:8080"),
	)
	.await;
	assert_eq!(response.status(), 303);
	assert!(
		response.headers()["location"]
			.to_str()
			.unwrap()
			.starts_with("/sign-in?state="),
		"{pool}"
	);
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
async fn sign_in(app: &common::TestApplication, pool: &str, name: &str) -> (String, String) {
	sign_in_as(app, pool, "person", name).await
}
async fn sign_in_as(
	app: &common::TestApplication,
	pool: &str,
	uid: &str,
	name: &str,
) -> (String, String) {
	let (state, cookie) = transaction(app, pool).await;
	let response = exchange(
		app,
		&state,
		&cookie,
		&signed_token(pool, uid, name, Utc::now().timestamp(), "password"),
		"https://127.0.0.1:8080",
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
async fn discover(
	app: &common::TestApplication,
	body: &str,
	origin: Option<&str>,
) -> reqwest::Response {
	let mut request = browser()
		.post(app.url("/auth/login"))
		.header("content-type", "application/x-www-form-urlencoded")
		.body(body.to_owned());
	if let Some(origin) = origin {
		request = request.header("origin", origin);
	}
	request.send().await.unwrap()
}
async fn pending_transactions(f: &Federation) -> i64 {
	let query = Query::select()
		.expr(Expr::cust("count(*)"))
		.from(Alias::new("dashboard_login_transactions"))
		.to_string(PostgresQueryBuilder);
	sqlx::query_scalar(&query)
		.fetch_one(f.store.pool.fixture_driver())
		.await
		.unwrap()
}

#[rstest::rstest]
#[tokio::test]
async fn sign_in_domain_routes_to_its_pool_and_unknown_inputs_are_indistinguishable(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	let app = common::application_with_settings(
		f.clone(),
		aidash_server::http::Settings {
			auth_discovery_burst: 1000,
			..Default::default()
		},
	)
	.await;
	const ORIGIN: &str = "https://127.0.0.1:8080";
	for (body, pool) in [
		("email=Alice%40ACME.example&return_to=%2Fsettings", "pool-a"),
		(
			"email=bob%40B%C3%9CCHER.example&return_to=%2Fsettings",
			"pool-b",
		),
	] {
		let response = discover(&app, body, Some(ORIGIN)).await;
		assert_eq!(response.status(), 303, "{body}");
		assert_eq!(response.headers()["cache-control"], "no-store");
		let location = response.headers()["location"].to_str().unwrap();
		let state = location.strip_prefix("/sign-in?state=").unwrap().to_owned();
		let cookie = response.headers()["set-cookie"]
			.to_str()
			.unwrap()
			.split(';')
			.next()
			.unwrap()
			.to_owned();
		let client: Value = browser()
			.get(app.url(format!("/auth/gcip/transaction?state={state}")))
			.header("cookie", &cookie)
			.send()
			.await
			.unwrap()
			.json()
			.await
			.unwrap();
		assert_eq!(client["tenant_id"], pool);
		assert!(!client.to_string().contains("acme"), "{client}");
		let exchanged = exchange(
			&app,
			&state,
			&cookie,
			&token(pool, "Person", Utc::now().timestamp()),
			ORIGIN,
		)
		.await;
		assert_eq!(exchanged.status(), 200);
	}
	let before = pending_transactions(&f).await;
	let mut unknown = Vec::new();
	for body in [
		"email=alice%40unmapped.example&return_to=%2Fsettings",
		"email=alice%40eng.acme.example&return_to=%2Fsettings",
		"email=not-an-email&return_to=%2Fsettings",
		"email=%40acme.example&return_to=%2Fsettings",
		// Mapped domains do not rescue a malformed mailbox.
		"email=bad+local%40acme.example&return_to=%2Fsettings",
		"email=.alice%40acme.example&return_to=%2Fsettings",
		"email=alice..bob%40acme.example&return_to=%2Fsettings",
		// A literal `%61cme.example` must not percent-decode into the mapping.
		"email=alice%40%2561cme.example&return_to=%2Fsettings",
		"return_to=%2Fsettings",
	] {
		unknown.push((body, discover(&app, body, Some(ORIGIN)).await));
	}
	let mut bodies = Vec::new();
	for (input, response) in unknown {
		assert_eq!(response.status(), 303, "{input}");
		assert_eq!(
			response.headers()["location"],
			"/sign-in?error=not_found&return_to=%2Fsettings",
			"{input}"
		);
		assert_eq!(response.headers()["cache-control"], "no-store", "{input}");
		assert!(!response.headers().contains_key("set-cookie"), "{input}");
		bodies.push(response.bytes().await.unwrap());
	}
	assert!(bodies.windows(2).all(|pair| pair[0] == pair[1]));
	assert_eq!(pending_transactions(&f).await, before);
	// Discovery is a browser form; a foreign or missing Origin is refused
	// before any routing decision, for mapped and unmapped domains alike.
	for origin in [None, Some("https://attacker.example")] {
		for body in [
			"email=alice%40acme.example",
			"email=alice%40unmapped.example",
		] {
			assert_eq!(discover(&app, body, origin).await.status(), 403);
		}
	}
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn discovery_has_its_own_rate_budget(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	let app = common::application_with_settings(
		f.clone(),
		aidash_server::http::Settings {
			auth_discovery_burst: 2,
			auth_discovery_period: std::time::Duration::from_secs(600),
			..Default::default()
		},
	)
	.await;
	let origin = Some("https://127.0.0.1:8080");
	assert_eq!(
		discover(&app, "email=a%40unmapped.example", origin)
			.await
			.status(),
		303
	);
	assert_eq!(
		discover(&app, "email=a%40acme.example", origin)
			.await
			.status(),
		303
	);
	for limited in [
		discover(&app, "email=a%40unmapped.example", origin).await,
		discover(&app, "email=a%40acme.example", origin).await,
	] {
		assert_eq!(limited.status(), 429);
	}
	// Exhausted discovery leaves ordinary sign-in endpoints available. A
	// Tenant name no longer selects a sign-in, so `org` is no lookup.
	for path in [
		"/auth/config",
		"/auth/login?return_to=%2Fsettings",
		"/auth/login?org=acme",
	] {
		let response = browser().get(app.url(path)).send().await.unwrap();
		assert_ne!(response.status(), 429, "{path}");
	}
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn provider_allowlist_is_enforced_for_an_existing_uid_before_admin_io(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime,
		status,
		_admin,
	} = gcip;
	let (mut f, url, schema) = runtime.parts();
	let app = common::application(f.clone()).await;
	sign_in(&app, "pool-a", "Person").await;
	reconfigure(&mut f, |gcip| {
		gcip.providers.insert(
			"pool-a".into(),
			vec!["oidc.company".into(), "saml.company".into()],
		);
	});
	let changed = common::application(f.clone()).await;
	status.outage.store(true, Ordering::SeqCst);
	for provider in ["password", "google.com", "custom"] {
		let (state, cookie) = transaction(&changed, "pool-a").await;
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
				"https://127.0.0.1:8080"
			)
			.await
			.status(),
			403,
			"{provider}"
		);
	}
	status.outage.store(false, Ordering::SeqCst);
	for provider in ["oidc.company", "saml.company"] {
		let (state, cookie) = transaction(&changed, "pool-a").await;
		assert_eq!(
			exchange(
				&changed,
				&state,
				&cookie,
				&token_with_provider("pool-a", "Person", Utc::now().timestamp(), provider),
				"https://127.0.0.1:8080"
			)
			.await
			.status(),
			200,
			"{provider}"
		);
	}
	// An explicitly empty list disables every method rather than selecting defaults.
	reconfigure(&mut f, |gcip| {
		gcip.providers.insert("pool-a".into(), vec![]);
	});
	let disabled = common::application(f.clone()).await;
	let (state, cookie) = transaction(&disabled, "pool-a").await;
	assert_eq!(
		exchange(
			&disabled,
			&state,
			&cookie,
			&token_with_provider("pool-a", "Person", Utc::now().timestamp(), "saml.company"),
			"https://127.0.0.1:8080"
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
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
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
	let page = browser().get(app.url("/auth/login")).send().await.unwrap();
	assert!(
		page.headers()["location"]
			.to_str()
			.unwrap()
			.starts_with("/sign-in?")
	);
	let (state, cookie) = transaction(&app, "pool-a").await;
	let valid = token("pool-a", "Person", Utc::now().timestamp());
	for (browser_cookie, jwt, origin, expected) in [
		(
			cookie.as_str(),
			valid.as_str(),
			"https://attacker.example",
			403,
		),
		(
			"__Host-aidash-login=wrong",
			valid.as_str(),
			"https://127.0.0.1:8080",
			401,
		),
		(
			cookie.as_str(),
			"invalid-token",
			"https://127.0.0.1:8080",
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
			exchange(&app, &state, &cookie, &invalid, "https://127.0.0.1:8080")
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
			"https://127.0.0.1:8080"
		)
		.await
		.status(),
		401
	);
	assert_eq!(
		exchange(&app, &state, &cookie, &valid, "https://127.0.0.1:8080")
			.await
			.status(),
		200
	);
	assert_eq!(
		exchange(&app, &state, &cookie, &valid, "https://127.0.0.1:8080")
			.await
			.status(),
		401
	);
	let (state, cookie) = transaction(&app, "pool-a").await;
	let expire = Query::update()
		.table(Alias::new("dashboard_login_transactions"))
		.value_expr(
			Alias::new("expires_at"),
			Expr::cust("clock_timestamp()-interval '1 second'"),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&expire)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(
		exchange(&app, &state, &cookie, &valid, "https://127.0.0.1:8080")
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
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime,
		status,
		_admin,
	} = gcip;
	let (f, url, schema) = runtime.parts();
	let app = common::application(f.clone()).await;
	let (cookie, csrf) = sign_in(&app, "pool-a", "First").await;
	let registrations = browser()
		.post(app.url("/auth/registration"))
		.header("cookie", &cookie)
		.header("origin", "https://127.0.0.1:8080")
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
	let _ = sign_in(&app, "pool-b", "Other").await;
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
	let _ = sign_in(&app, "pool-a", "Changed").await;
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
	// A Deployment Administrator's rejection still records the Tenant the
	// Binding addressed the request to.
	let rejected_for: Option<String> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("tenant"))
			.from(Alias::new("dashboard_administration_history"))
			.and_where(Expr::cust("action = 'registration.reject'"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(rejected_for.as_deref(), Some("acme"));
	let force_stale = Query::update()
		.table(Alias::new("dashboard_identities"))
		.value_expr(
			Alias::new("last_valid_at"),
			Expr::cust("clock_timestamp()-interval '6 minutes'"),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&force_stale)
		.execute(f.store.pool.fixture_driver())
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
		.execute(f.store.pool.fixture_driver())
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
	let (state, bound) = transaction(&app, "pool-a").await;
	assert_eq!(
		exchange(
			&app,
			&state,
			&bound,
			&token("pool-a", "Denied", Utc::now().timestamp()),
			"https://127.0.0.1:8080"
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
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
	use sha2::{Digest, Sha256};
	let GcipFixture {
		runtime,
		status,
		_admin,
	} = gcip;
	let (f, url, schema) = runtime.parts();
	let app = common::application(f.clone()).await;
	let (cookie, csrf) = sign_in(&app, "pool-a", "Person").await;
	let cookie = format!("{cookie}; aidash-csrf={csrf}");
	let verifier = "a".repeat(64);
	let state = "b".repeat(64);
	let redirect = "http://127.0.0.1:43217/callback";
	let start:Value=browser().post(app.url("/auth/desktop/start")).json(&json!({"redirect_uri":redirect,"state":state,"code_challenge":URL_SAFE_NO_PAD.encode(Sha256::digest(&verifier))})).send().await.unwrap().json().await.unwrap();
	let authorization = start["authorization_url"]
		.as_str()
		.unwrap()
		.strip_prefix("https://127.0.0.1:8080")
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
		.header("origin", "https://127.0.0.1:8080")
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
		.fetch_all(f.store.pool.fixture_driver())
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
		.execute(f.store.pool.fixture_driver())
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
async fn a_pending_request_is_not_reused_once_its_pool_is_bound_to_another_tenant(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (mut f, url, schema) = runtime.parts();
	let app = common::application(f.clone()).await;
	let submit = async |app: &common::TestApplication, cookie: &str, csrf: &str| -> Value {
		let response = browser()
			.post(app.url("/auth/registration"))
			.header("cookie", cookie)
			.header("origin", "https://127.0.0.1:8080")
			.header("x-aidash-csrf", csrf)
			.send()
			.await
			.unwrap();
		assert_eq!(response.status(), 200);
		response.json().await.unwrap()
	};
	let (cookie, csrf) = sign_in(&app, "pool-a", "Person").await;
	let first = submit(&app, &cookie, &csrf).await;
	assert_eq!(first["tenant"], "acme");
	// The pool moves to another Tenant while the request is pending. That
	// changes the identity's pool status, so it signs in again only after a
	// Deployment Administrator restore (#240).
	common::tenant_root(&f, "third").await;
	reconfigure(&mut f, |gcip| {
		gcip.tenant_bindings.insert("pool-a".into(), "third".into());
	});
	let moved = common::application(f.clone()).await;
	let (state, transaction_cookie) = transaction(&moved, "pool-a").await;
	assert_eq!(
		exchange(
			&moved,
			&state,
			&transaction_cookie,
			&signed_token(
				"pool-a",
				"person",
				"Person",
				Utc::now().timestamp(),
				"password"
			),
			"https://127.0.0.1:8080"
		)
		.await
		.status(),
		403
	);
	assert!(identity_disabled(&f, "person").await);
	let id: uuid::Uuid = scalar(
		&f,
		"SELECT id FROM dashboard_identities WHERE subject = 'person'",
	)
	.await;
	let restored = browser()
		.post(moved.url(format!("/api/dashboard/identities/{id}/restore")))
		.bearer_auth(&f.config.api_token)
		.send()
		.await
		.unwrap();
	assert_eq!(restored.status(), 204);
	let (cookie, csrf) = sign_in(&moved, "pool-a", "Person").await;
	let second = submit(&moved, &cookie, &csrf).await;
	assert_ne!(second["id"], first["id"]);
	assert_eq!(second["tenant"], "third");
	assert_eq!(second["status"], "pending");
	// It ended when it was replaced, not at the deadline it never reached,
	// which display retention would otherwise treat as its end.
	let (ended, ended_early): (String, bool) = sqlx::query_as(
		&Query::select()
			.column(Alias::new("status"))
			.expr(Expr::cust("expires_at <= clock_timestamp()"))
			.from(Alias::new("dashboard_registration_requests"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(
				uuid::Uuid::parse_str(first["id"].as_str().unwrap()).unwrap(),
			)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(ended, "expired");
	assert!(ended_early);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn removing_binding_disables_a_fresh_identity_at_its_next_boundary(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (mut f, url, schema) = runtime.parts();
	let app = common::application(f.clone()).await;
	let (cookie, _) = sign_in(&app, "pool-a", "Person").await;
	// Settings validation removes a pool's Sign-in Domains with its Binding.
	reconfigure(&mut f, |gcip| {
		gcip.tenant_bindings.remove("pool-a");
		gcip.sign_in_domains.retain(|_, pool| pool != "pool-a");
	});
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
#[tokio::test]
async fn an_identity_remembers_the_pool_status_it_was_first_admitted_under(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (mut f, url, schema) = runtime.parts();
	let admissions = async |f: &Federation| -> Vec<(String, Option<String>)> {
		sqlx::query_as("SELECT gcip_tenant, tenant FROM gcip_admissions")
			.fetch_all(f.store.pool.fixture_driver())
			.await
			.unwrap()
	};
	let app = common::application(f.clone()).await;
	sign_in(&app, "pool-a", "Person").await;
	assert_eq!(
		admissions(&f).await,
		[("pool-a".to_owned(), Some("acme".to_owned()))]
	);
	let again = common::application(f.clone()).await;
	sign_in(&again, "pool-a", "Person").await;
	assert_eq!(
		admissions(&f).await,
		[("pool-a".to_owned(), Some("acme".to_owned()))],
		"a later sign-in keeps the first admission"
	);
	// A Binding moved to another Tenant is a new status: the identity waits
	// for a restore, and the record names the Tenant it will be admitted to.
	common::tenant_root(&f, "third").await;
	reconfigure(&mut f, |gcip| {
		gcip.tenant_bindings.insert("pool-a".into(), "third".into());
	});
	f.config
		.sign_in
		.as_ref()
		.unwrap()
		.reconcile(&f)
		.await
		.unwrap();
	assert!(identity_disabled(&f, "person").await);
	assert_eq!(
		admissions(&f).await,
		[("pool-a".to_owned(), Some("third".to_owned()))]
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[case::login("login")]
#[case::refresh("refresh")]
#[case::install("install")]
#[tokio::test]
async fn removed_binding_login_disables_an_inactive_identity_and_its_existing_authority(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
	#[case] boundary: &str,
) {
	let GcipFixture {
		runtime,
		status,
		_admin,
	} = gcip;
	let (mut f, url, schema) = runtime.parts();
	let policy = serde_json::from_value(
		json!({"tenant":"acme","subjects":{"alice":{"kind":"user"}},"policies":[]}),
	)
	.unwrap();
	common::tenant_root_in(f.store.pool.fixture_driver(), "acme").await;
	aidash_server::authorization::Authorization {
		pool: f.store.pool.clone(),
	}
	.replace("acme", 0, policy, "deployment_administrator")
	.await
	.unwrap();
	let app = common::application(f.clone()).await;
	let (cookie, csrf) = sign_in(&app, "pool-a", "Person").await;
	let registration: Value = browser()
		.post(app.url("/auth/registration"))
		.header("cookie", cookie)
		.header("origin", "https://127.0.0.1:8080")
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
			"/api/dashboard/identities/{identity}/deployment-administrator-grant"
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
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	let (state, cookie) = transaction(&app, "pool-a").await;
	// Terraform removes a pool's Binding and its Sign-in Domains together.
	reconfigure(&mut f, |gcip| {
		gcip.tenant_bindings.remove("pool-a");
		gcip.sign_in_domains.retain(|_, pool| pool != "pool-a");
	});
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
			"https://127.0.0.1:8080"
		)
		.await
		.status(),
		401
	);
	assert!(
		sqlx::query_scalar::<_, Option<chrono::DateTime<Utc>>>(&disabled)
			.fetch_one(f.store.pool.fixture_driver())
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
				"https://127.0.0.1:8080"
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
		// Only the Enterprise build's own settings source carries the GCIP
		// settings, as the managed file of a Cloud Environment does.
		let managed = tempfile::NamedTempFile::new().unwrap();
		std::fs::write(
			managed.path(),
			serde_json::to_vec(&json!({"dashboard": {"gcip": gcip_settings()}})).unwrap(),
		)
		.unwrap();
		// SAFETY: no other test of this binary reads the variable.
		unsafe { std::env::set_var("AIDASH_GCIP_SETTINGS", managed.path()) };
		let context = reinhardt::test::fixtures::injection_context::default();
		let installed =
			aidash_server::bootstrap::initialize(&context, &settings, f.store.pool.connection())
				.await;
		unsafe { std::env::remove_var("AIDASH_GCIP_SETTINGS") };
		installed.unwrap().store.control_pool.close().await;
	} else {
		let (stop, stopping) = tokio::sync::watch::channel(false);
		let runtime = f.clone();
		let mut refresh = AbortOnDrop(tokio::spawn(async move {
			aidash_server::dashboard_auth::refresh_active(runtime, stopping)
				.await
				.unwrap();
		}));
		tokio::time::timeout(std::time::Duration::from_secs(5), async {
			loop {
				if sqlx::query_scalar::<_, Option<chrono::DateTime<Utc>>>(&disabled)
					.fetch_one(f.store.pool.fixture_driver())
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
			.fetch_one(f.store.pool.fixture_driver())
			.await
			.unwrap()
			.is_some()
	);
	for table in [
		"dashboard_mappings",
		"dashboard_deployment_administrator_grants",
	] {
		let enabled = Query::select()
			.column(Alias::new("enabled"))
			.from(Alias::new(table))
			.and_where(Expr::col("identity_id").eq(Expr::val(identity)))
			.to_string(PostgresQueryBuilder);
		// Disablement preserves Deployment Administrator-managed approvals, but gates all of
		// them behind an explicit Identity restore rather than a fresh login.
		assert!(
			sqlx::query_scalar::<_, bool>(&enabled)
				.fetch_one(f.store.pool.fixture_driver())
				.await
				.unwrap(),
			"{table} approval must still exist while the Identity is disabled"
		);
	}
	reconfigure(&mut f, |gcip| {
		gcip.tenant_bindings.insert("pool-a".into(), "acme".into());
		gcip.sign_in_domains
			.insert("acme.example".into(), "pool-a".into());
	});
	status.outage.store(false, Ordering::SeqCst);
	let restored_binding = common::application(f.clone()).await;
	let (state, cookie) = transaction(&restored_binding, "pool-a").await;
	assert_eq!(
		exchange(
			&restored_binding,
			&state,
			&cookie,
			&token("pool-a", "Person", Utc::now().timestamp()),
			"https://127.0.0.1:8080"
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
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	for tenant in ["acme", "other"] {
		let policy = serde_json::from_value(
			json!({"tenant":tenant,"subjects":{"alice":{"kind":"user"}},"policies":[]}),
		)
		.unwrap();
		common::tenant_root_in(f.store.pool.fixture_driver(), tenant).await;
		aidash_server::authorization::Authorization {
			pool: f.store.pool.clone(),
		}
		.replace(tenant, 0, policy, "deployment_administrator")
		.await
		.unwrap();
	}
	let app = common::application(f.clone()).await;
	let (cookie, csrf) = sign_in(&app, "pool-a", "Person").await;
	let registration: Value = browser()
		.post(app.url("/auth/registration"))
		.header("cookie", &cookie)
		.header("origin", "https://127.0.0.1:8080")
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
	let selector = format!(
		"mapping:{}:{}",
		mapping["tenant"].as_str().unwrap(),
		mapping["id"].as_str().unwrap()
	);
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
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	let mut boundary = f.store.pool.fixture_driver().begin().await.unwrap();
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
		.fetch_one(f.store.pool.fixture_driver())
		.await
		.unwrap();
	// A Mapping and its credential move together: each names the other's Tenant.
	common::tenant_root_in(f.store.pool.fixture_driver(), "other").await;
	sqlx::query(
		"WITH moved AS (UPDATE authorization_credentials SET tenant = 'other' WHERE id = $1) \
		 UPDATE dashboard_mappings SET tenant = 'other' WHERE id = $2",
	)
	.bind(credential)
	.bind(id)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
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
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	let app = common::application(f.clone()).await;
	let (cookie, csrf) = sign_in(&app, "pool-a", "Original").await;
	let previous: Value = browser()
		.post(app.url("/auth/registration"))
		.header("cookie", cookie)
		.header("origin", "https://127.0.0.1:8080")
		.header("x-aidash-csrf", csrf)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	// A request expires no earlier than its creation, after the sign-in that made it.
	let expire = Query::update()
		.table(Alias::new("dashboard_registration_requests"))
		.value_expr(
			Alias::new("expires_at"),
			Expr::col(Alias::new("created_at")),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&expire)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	let (cookie, csrf) = sign_in(&app, "pool-a", "Freshly authenticated").await;
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
		.header("origin", "https://127.0.0.1:8080")
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
		.execute(f.store.pool.fixture_driver())
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
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	let app = common::application(f.clone()).await;
	let (cookie, csrf) = sign_in(&app, "pool-a", "Person").await;
	let registration: Value = browser()
		.post(app.url("/auth/registration"))
		.header("cookie", &cookie)
		.header("origin", "https://127.0.0.1:8080")
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
			Expr::col(Alias::new("created_at")),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&expire)
		.execute(f.store.pool.fixture_driver())
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

/// Runs exactly one periodic refresh pass, which also enforces display retention.
async fn sweep(f: &Federation) {
	let (stop, stopping) = tokio::sync::watch::channel(false);
	stop.send(true).unwrap();
	aidash_server::dashboard_auth::refresh_active(f.clone(), stopping)
		.await
		.unwrap();
}
async fn age(f: &Federation, table: &str, column: &str, interval: &str) {
	let update = Query::update()
		.table(Alias::new(table))
		.value_expr(
			Alias::new(column),
			Expr::cust(format!("clock_timestamp()-interval '{interval}'")),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
}
async fn deployment_administrator_get(
	app: &common::TestApplication,
	f: &Federation,
	path: &str,
) -> Value {
	let response = browser()
		.get(app.url(path))
		.bearer_auth(&f.config.api_token)
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 200);
	response.json().await.unwrap()
}
async fn only_identity_path(app: &common::TestApplication, f: &Federation) -> String {
	let identities = deployment_administrator_get(app, f, "/api/dashboard/identities").await;
	let [identity] = identities.as_array().unwrap().as_slice() else {
		panic!("expected one External Identity: {identities}");
	};
	format!(
		"/api/dashboard/identities/{}",
		identity["id"].as_str().unwrap()
	)
}

#[rstest::rstest]
#[tokio::test]
async fn abandoned_sign_in_keeps_display_attributes_only_while_its_session_lives(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	let app = common::application(f.clone()).await;
	// The applicant signs in and leaves without submitting a Registration Request.
	sign_in(&app, "pool-a", "Abandoned").await;
	let path = only_identity_path(&app, &f).await;
	age(
		&f,
		"dashboard_identities",
		"display_observed_at",
		"11 minutes",
	)
	.await;
	sweep(&f).await;
	let view = deployment_administrator_get(&app, &f, &path).await;
	assert_eq!(
		view["display_name"], "Abandoned",
		"the live session retains them"
	);
	assert_eq!(view["verified_email"], "person@example.test");
	// The idle limit is 30 minutes; the abandoned session no longer counts.
	age(&f, "dashboard_sessions", "last_activity_at", "31 minutes").await;
	sweep(&f).await;
	let view = deployment_administrator_get(&app, &f, &path).await;
	assert!(view["display_name"].is_null() && view["verified_email"].is_null());
	assert!(
		view["display_erased_at"].is_null(),
		"retention clearing is not a Display Erasure"
	);
	sign_in(&app, "pool-a", "Returned").await;
	assert_eq!(
		deployment_administrator_get(&app, &f, &path).await["display_name"],
		"Returned"
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn display_sweep_rechecks_retention_after_waiting_for_a_fresh_sign_in(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	let app = common::application(f.clone()).await;
	let (cookie, csrf) = sign_in(&app, "pool-a", "Stale").await;
	let submitted = browser()
		.post(app.url("/auth/registration"))
		.header("cookie", &cookie)
		.header("origin", "https://127.0.0.1:8080")
		.header("x-aidash-csrf", csrf)
		.send()
		.await
		.unwrap();
	assert_eq!(submitted.status(), 200);
	let path = only_identity_path(&app, &f).await;
	// Nothing retains the stale attributes: the request has expired, the sign-in
	// is old, and its session has ended.
	let expire = Query::update()
		.table(Alias::new("dashboard_registration_requests"))
		.value_expr(
			Alias::new("expires_at"),
			Expr::col(Alias::new("created_at")),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&expire)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	age(
		&f,
		"dashboard_identities",
		"display_observed_at",
		"11 minutes",
	)
	.await;
	age(&f, "dashboard_sessions", "revoked_at", "0 seconds").await;
	// A fresh sign-in holds the identity lock while it records new claims.
	let mut tx = f.store.pool.fixture_driver().begin().await.unwrap();
	let lock = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("dashboard_identities"))
		.lock(reinhardt::query::LockType::Update)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&lock).fetch_one(&mut *tx).await.unwrap();
	let holder: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
		.fetch_one(&mut *tx)
		.await
		.unwrap();
	let pass = tokio::spawn({
		let f = f.clone();
		async move { sweep(&f).await }
	});
	// The sweep runs in Deployment Scope, which renames its application_name,
	// so find it by the lock it waits for.
	let waiters = Query::select()
		.expr(Expr::cust("count(*)"))
		.from(Alias::new("pg_stat_activity"))
		.and_where(Expr::cust_with_values(
			"? = ANY(pg_blocking_pids(pid))",
			[holder],
		))
		.and_where(Expr::col(Alias::new("query")).like("%dashboard_identities%"))
		.to_string(PostgresQueryBuilder);
	tokio::time::timeout(std::time::Duration::from_secs(10), async {
		while sqlx::query_scalar::<_, i64>(&waiters)
			.fetch_one(f.store.pool.fixture_driver())
			.await
			.unwrap() == 0
		{
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	})
	.await
	.expect("the sweep must wait for the identity lock");
	let fresh = Query::update()
		.table(Alias::new("dashboard_identities"))
		.value_expr(Alias::new("display_name"), Expr::val("Fresh"))
		.value_expr(
			Alias::new("display_observed_at"),
			Expr::cust("clock_timestamp()"),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&fresh).execute(&mut *tx).await.unwrap();
	tx.commit().await.unwrap();
	tokio::time::timeout(std::time::Duration::from_secs(10), pass)
		.await
		.unwrap()
		.unwrap();
	let view = deployment_administrator_get(&app, &f, &path).await;
	assert_eq!(
		view["display_name"], "Fresh",
		"the sweep must not erase claims recorded while it waited"
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn display_erasure_is_permanent_and_keeps_the_identity_authority(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	let policy = serde_json::from_value(
		json!({"tenant":"acme","subjects":{"alice":{"kind":"user"}},"policies":[]}),
	)
	.unwrap();
	common::tenant_root_in(f.store.pool.fixture_driver(), "acme").await;
	aidash_server::authorization::Authorization {
		pool: f.store.pool.clone(),
	}
	.replace("acme", 0, policy, "deployment_administrator")
	.await
	.unwrap();
	let app = common::application(f.clone()).await;
	let (cookie, csrf) = sign_in(&app, "pool-a", "Person").await;
	let registration: Value = browser()
		.post(app.url("/auth/registration"))
		.header("cookie", &cookie)
		.header("origin", "https://127.0.0.1:8080")
		.header("x-aidash-csrf", csrf)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
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
	let mapping: Value = approved.json().await.unwrap();
	let selector = format!(
		"mapping:{}:{}",
		mapping["tenant"].as_str().unwrap(),
		mapping["id"].as_str().unwrap()
	);
	let identity = registration["identity_id"].as_str().unwrap();
	let path = format!("/api/dashboard/identities/{identity}");
	assert_eq!(
		deployment_administrator_get(&app, &f, &path).await["display_name"],
		"Person"
	);
	let erase = || {
		browser()
			.post(app.url(format!("{path}/display-erasure")))
			.bearer_auth(&f.config.api_token)
			.send()
	};
	let erased = erase().await.unwrap();
	assert_eq!(erased.status(), 200);
	let erased: Value = erased.json().await.unwrap();
	assert!(erased["display_name"].is_null() && erased["verified_email"].is_null());
	assert_eq!(
		erased["display_erased_by"],
		"deployment-administrator-bearer"
	);
	assert!(erased["display_erased_at"].is_string());
	let repeated: Value = erase().await.unwrap().json().await.unwrap();
	assert_eq!(
		repeated["display_erased_at"], erased["display_erased_at"],
		"a repeated erasure keeps the first trace"
	);
	// Later sign-ins no longer record the attributes, yet the Mapping still decides.
	let (cookie, _) = sign_in(&app, "pool-a", "Renamed").await;
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
	sweep(&f).await;
	let view = deployment_administrator_get(&app, &f, &path).await;
	assert!(view["display_name"].is_null() && view["verified_email"].is_null());
	assert_eq!(view["display_erased_at"], erased["display_erased_at"]);
	for listing in ["/api/dashboard/identities", "/api/dashboard/registrations"] {
		let body = deployment_administrator_get(&app, &f, listing)
			.await
			.to_string();
		assert!(
			!body.contains("person@example.test") && !body.contains("Renamed"),
			"{listing} returned erased attributes"
		);
	}
	let mappings = deployment_administrator_get(&app, &f, "/api/dashboard/mappings").await;
	assert!(
		mappings
			.as_array()
			.unwrap()
			.iter()
			.any(|row| row["id"] == mapping["id"] && row["enabled"] == true)
	);
	assert_eq!(
		browser()
			.post(app.url(format!(
				"/api/dashboard/identities/{}/display-erasure",
				uuid::Uuid::new_v4()
			)))
			.bearer_auth(&f.config.api_token)
			.send()
			.await
			.unwrap()
			.status(),
		404
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn rejected_sign_in_of_a_disabled_identity_records_no_display_attributes(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	let app = common::application(f.clone()).await;
	sign_in(&app, "pool-a", "Person").await;
	let path = only_identity_path(&app, &f).await;
	age(&f, "dashboard_identities", "disabled_at", "0 seconds").await;
	sweep(&f).await;
	let view = deployment_administrator_get(&app, &f, &path).await;
	assert!(view["display_name"].is_null() && view["verified_email"].is_null());
	// The provider still verifies the person, but the local identity stays disabled.
	let (state, cookie) = transaction(&app, "pool-a").await;
	let rejected = exchange(
		&app,
		&state,
		&cookie,
		&token("pool-a", "Again", Utc::now().timestamp()),
		"https://127.0.0.1:8080",
	)
	.await;
	assert_eq!(rejected.status(), 403);
	let view = deployment_administrator_get(&app, &f, &path).await;
	assert!(
		view["display_name"].is_null() && view["verified_email"].is_null(),
		"a rejected sign-in must not restore Display Attributes: {view}"
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn display_retention_runs_after_dashboard_sign_in_is_unconfigured(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	let app = common::application(f.clone()).await;
	sign_in(&app, "pool-a", "Person").await;
	let path = only_identity_path(&app, &f).await;
	// The deployment restarts without its optional sign-in configuration. Neither
	// the still-live session nor the sign-in from moments ago retains anything,
	// because no session is usable or can still finish starting.
	let mut unconfigured = f.clone();
	unconfigured.config.sign_in = None;
	let (stop, stopping) = tokio::sync::watch::channel(false);
	stop.send(true).unwrap();
	aidash_server::dashboard_auth::enforce_display_retention(unconfigured, stopping)
		.await
		.unwrap();
	let view = deployment_administrator_get(&app, &f, &path).await;
	assert!(view["display_name"].is_null() && view["verified_email"].is_null());
	assert!(view["display_erased_at"].is_null());
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn bearer_deployment_administrator_rejects_a_request_after_dashboard_sign_in_is_unconfigured(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	let app = common::application(f.clone()).await;
	let (cookie, csrf) = sign_in(&app, "pool-a", "Person").await;
	let registration: Value = browser()
		.post(app.url("/auth/registration"))
		.header("cookie", &cookie)
		.header("origin", "https://127.0.0.1:8080")
		.header("x-aidash-csrf", csrf)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	let mut unconfigured = f.clone();
	unconfigured.config.sign_in = None;
	let restarted = common::application(unconfigured.clone()).await;
	let rejected = browser()
		.post(restarted.url(format!(
			"/api/dashboard/registrations/{}/reject",
			registration["id"].as_str().unwrap()
		)))
		.bearer_auth(&f.config.api_token)
		.send()
		.await
		.unwrap();
	assert_eq!(rejected.status(), 200);
	let rejected: Value = rejected.json().await.unwrap();
	assert_eq!(rejected["status"], "rejected");
	let identity = registration["identity_id"].as_str().unwrap();
	let view = deployment_administrator_get(
		&restarted,
		&unconfigured,
		&format!("/api/dashboard/identities/{identity}"),
	)
	.await;
	assert!(view["display_name"].is_null() && view["verified_email"].is_null());
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn rejected_sign_in_of_a_disabled_identity_does_not_extend_display_retention(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	let app = common::application(f.clone()).await;
	sign_in(&app, "pool-a", "Person").await;
	let path = only_identity_path(&app, &f).await;
	// The identity is disabled before a sweep clears its stale attributes.
	age(
		&f,
		"dashboard_identities",
		"display_observed_at",
		"11 minutes",
	)
	.await;
	age(&f, "dashboard_sessions", "revoked_at", "0 seconds").await;
	age(&f, "dashboard_identities", "disabled_at", "0 seconds").await;
	let (state, cookie) = transaction(&app, "pool-a").await;
	let rejected = exchange(
		&app,
		&state,
		&cookie,
		&token("pool-a", "Again", Utc::now().timestamp()),
		"https://127.0.0.1:8080",
	)
	.await;
	assert_eq!(rejected.status(), 403);
	// Restored before the next sweep, the rejected sign-in must not retain them.
	let restore = Query::update()
		.table(Alias::new("dashboard_identities"))
		.value_expr(Alias::new("disabled_at"), Expr::cust("null"))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&restore)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	sweep(&f).await;
	let view = deployment_administrator_get(&app, &f, &path).await;
	assert!(
		view["display_name"].is_null() && view["verified_email"].is_null(),
		"a rejected sign-in must not become a retention basis: {view}"
	);
	common::cleanup(f, &url, &schema).await;
}

/// One signed-in browser acting through a selected context.
struct Browser {
	cookie: String,
	csrf: String,
	context: String,
}
impl Browser {
	fn get(&self, app: &common::TestApplication, path: &str) -> reqwest::RequestBuilder {
		browser()
			.get(app.url(path))
			.header("cookie", &self.cookie)
			.header("x-aidash-context", &self.context)
	}
	fn send(
		&self,
		app: &common::TestApplication,
		method: reqwest::Method,
		path: &str,
	) -> reqwest::RequestBuilder {
		browser()
			.request(method, app.url(path))
			.header("cookie", &self.cookie)
			.header("origin", "https://127.0.0.1:8080")
			.header("x-aidash-csrf", &self.csrf)
			.header("x-aidash-context", &self.context)
	}
}

async fn register(app: &common::TestApplication, cookie: &str, csrf: &str) -> Value {
	let response = browser()
		.post(app.url("/auth/registration"))
		.header("cookie", cookie)
		.header("origin", "https://127.0.0.1:8080")
		.header("x-aidash-csrf", csrf)
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 200);
	response.json().await.unwrap()
}

/// Signs in, registers and lets the Deployment Administrator bearer approve into `subject`.
async fn mapped_browser(
	app: &common::TestApplication,
	f: &Federation,
	(org, pool, uid): (&str, &str, &str),
	subject: &str,
) -> (Browser, Value) {
	let (cookie, csrf) = sign_in_as(app, pool, uid, uid).await;
	let registration = register(app, &cookie, &csrf).await;
	let approved = browser()
		.post(app.url(format!(
			"/api/dashboard/registrations/{}/approve",
			registration["id"].as_str().unwrap()
		)))
		.bearer_auth(&f.config.api_token)
		.json(&json!({"tenant":org,"subject":subject}))
		.send()
		.await
		.unwrap();
	assert_eq!(approved.status(), 200);
	let mapping: Value = approved.json().await.unwrap();
	let context = format!(
		"mapping:{}:{}",
		mapping["tenant"].as_str().unwrap(),
		mapping["id"].as_str().unwrap()
	);
	(
		Browser {
			cookie,
			csrf,
			context,
		},
		mapping,
	)
}

/// `administrators` hold every Tenant Administrator action; only `members` is assignable.
fn administered_policy(tenant: &str, administrators: &[&str]) -> Value {
	let subjects: serde_json::Map<String, Value> = administrators
		.iter()
		.map(|subject| {
			(
				(*subject).to_owned(),
				json!({"kind": "user", "groups": ["administrators"]}),
			)
		})
		.collect();
	json!({
		"tenant": tenant,
		"roles": {"admin": {}, "member": {}},
		"groups": {
			"administrators": {"roles": ["admin"]},
			"members": {"roles": ["member"], "assignable": true}
		},
		"subjects": subjects,
		"policies": [{
			"id": "tenant-administrators",
			"effect": "allow",
			"subjects": {"groups": ["administrators"]},
			"actions": aidash_domain::identity::tenant_administration::ACTIONS,
			"resources": {"kinds": [
				"registration", "mapping", "subject", "invitation", "policy", "decision",
				"credential", "peer_mapping", "test_limits", "test_profile"
			]},
			"condition": null
		}, {
			// The last-administrator guard also needs someone who can delete the Tenant.
			"id": "tenant-deletion",
			"effect": "allow",
			"subjects": {"groups": ["administrators"]},
			"actions": ["tenant.delete"],
			"resources": {"kinds": ["tenant"]},
			"condition": null
		}]
	})
}

async fn replace_policy(f: &Federation, tenant: &str, revision: i64, policy: Value) {
	if revision == 0 {
		common::tenant_root(f, tenant).await;
	}
	aidash_server::authorization::Authorization {
		pool: f.store.pool.clone(),
	}
	.replace(
		tenant,
		revision,
		serde_json::from_value(policy).unwrap(),
		"deployment_administrator",
	)
	.await
	.unwrap();
}

/// `acme` is administered by `root` and `other` by `boss`.
async fn administered_tenants(f: &Federation) {
	for (tenant, administrator) in [("acme", "root"), ("other", "boss")] {
		replace_policy(f, tenant, 0, administered_policy(tenant, &[administrator])).await;
	}
}

async fn scalar<T>(f: &Federation, sql: &str) -> T
where
	T: for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres> + Send + Unpin,
{
	sqlx::query_scalar(sql)
		.fetch_one(f.store.pool.fixture_driver())
		.await
		.unwrap()
}

#[rstest::rstest]
#[tokio::test]
async fn tenant_administrator_cannot_read_or_approve_across_tenants(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	administered_tenants(&f).await;
	let app = common::application(f.clone()).await;
	let (admin, _) = mapped_browser(&app, &f, ("acme", "pool-a", "admin"), "root").await;
	let (other_admin, _) = mapped_browser(&app, &f, ("other", "pool-b", "boss"), "boss").await;
	let (cookie, csrf) = sign_in_as(&app, "pool-b", "outsider", "Outsider").await;
	let outsider = register(&app, &cookie, &csrf).await;
	let (cookie, csrf) = sign_in_as(&app, "pool-a", "newcomer", "Newcomer").await;
	let newcomer = register(&app, &cookie, &csrf).await;

	let overview: Value = admin
		.get(&app, "/api/tenants/acme/administration")
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(
		overview["actions"],
		json!(
			aidash_domain::identity::tenant_administration::ACTIONS
				.iter()
				.chain([&aidash_domain::identity::tenancy::TENANT_DELETE])
				.collect::<Vec<_>>()
		)
	);
	assert_eq!(overview["assignable_groups"], json!(["members"]));
	let listed = admin
		.get(&app, "/api/tenants/acme/registrations")
		.send()
		.await
		.unwrap();
	assert_eq!(listed.headers()["cache-control"], "no-store");
	let listed: Value = listed.json().await.unwrap();
	assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
	assert_eq!(listed[0]["id"], newcomer["id"]);
	assert_eq!(listed[0]["identity"]["display_name"], "Newcomer");
	assert_eq!(
		listed[0]["identity"]["verified_email"],
		"newcomer@example.test"
	);

	// Another Tenant's Registration Requests are neither readable nor approvable.
	for path in [
		"/api/tenants/other/registrations".to_owned(),
		"/api/tenants/other/mappings".to_owned(),
		"/api/tenants/other/administration".to_owned(),
	] {
		assert_eq!(
			admin.get(&app, &path).send().await.unwrap().status(),
			403,
			"{path}"
		);
	}
	let outsider_id = outsider["id"].as_str().unwrap();
	for (path, status) in [
		(
			format!("/api/tenants/other/registrations/{outsider_id}/approve"),
			403,
		),
		(
			format!("/api/tenants/acme/registrations/{outsider_id}/approve"),
			404,
		),
	] {
		assert_eq!(
			admin
				.send(&app, reqwest::Method::POST, &path)
				.json(&json!({"subject":"mallory","groups":["members"]}))
				.send()
				.await
				.unwrap()
				.status(),
			status,
			"{path}"
		);
	}
	assert_eq!(
		admin
			.send(
				&app,
				reqwest::Method::POST,
				&format!("/api/tenants/acme/registrations/{outsider_id}/reject"),
			)
			.send()
			.await
			.unwrap()
			.status(),
		404
	);
	// The other Tenant's own administrator still sees its request.
	let theirs: Value = other_admin
		.get(&app, "/api/tenants/other/registrations")
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(theirs[0]["id"], outsider["id"]);
	// Deployment Administrators and bearer Subject Credentials are never Tenant Administrators.
	assert_eq!(
		browser()
			.get(app.url("/api/tenants/acme/registrations"))
			.bearer_auth(&f.config.api_token)
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
async fn tenant_administrator_confers_only_assignable_groups_and_is_audited(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	administered_tenants(&f).await;
	let app = common::application(f.clone()).await;
	let (admin, admin_mapping) =
		mapped_browser(&app, &f, ("acme", "pool-a", "admin"), "root").await;
	let (cookie, csrf) = sign_in_as(&app, "pool-a", "newcomer", "Newcomer").await;
	let newcomer = register(&app, &cookie, &csrf).await;
	let approve = format!(
		"/api/tenants/acme/registrations/{}/approve",
		newcomer["id"].as_str().unwrap()
	);
	for (body, status) in [
		// An existing subject may hold authority outside Assignable Groups.
		(json!({"subject":"root"}), 409),
		(json!({"subject":"carol","groups":["administrators"]}), 400),
	] {
		assert_eq!(
			admin
				.send(&app, reqwest::Method::POST, &approve)
				.json(&body)
				.send()
				.await
				.unwrap()
				.status(),
			status,
			"{body}"
		);
	}
	let approved = admin
		.send(&app, reqwest::Method::POST, &approve)
		.json(&json!({"subject":"carol","groups":["members"]}))
		.send()
		.await
		.unwrap();
	assert_eq!(approved.status(), 200);
	let carol_mapping: Value = approved.json().await.unwrap();
	let snapshot: Value = browser()
		.get(app.url("/api/authorization/acme"))
		.bearer_auth(&f.config.api_token)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(
		snapshot["bundle"]["subjects"]["carol"],
		json!({"kind":"user","groups":["members"],"roles":[],"attributes":{},"enabled":true,"delegated_by":null})
	);
	let revision = snapshot["revision"].as_i64().unwrap();
	let admin_identity = admin_mapping["identity_id"].as_str().unwrap();
	assert_eq!(
		scalar::<String>(
			&f,
			"SELECT actor FROM authorization_revisions WHERE tenant='acme' ORDER BY revision DESC LIMIT 1"
		)
		.await,
		format!("oidc:{admin_identity}")
	);
	let carol = Browser {
		cookie,
		csrf,
		context: format!(
			"mapping:{}:{}",
			carol_mapping["tenant"].as_str().unwrap(),
			carol_mapping["id"].as_str().unwrap()
		),
	};
	assert_eq!(
		carol
			.get(&app, "/api/session")
			.send()
			.await
			.unwrap()
			.status(),
		200
	);
	// An ordinary member learns nothing about the policy, and is denied reads.
	let overview: Value = carol
		.get(&app, "/api/tenants/acme/administration")
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(
		overview,
		json!({"tenant":"acme","label":"acme","actions":[],"assignable_groups":[],"policy_revision":null})
	);
	assert_eq!(
		carol
			.get(&app, "/api/tenants/acme/registrations")
			.send()
			.await
			.unwrap()
			.status(),
		403
	);
	assert!(
		scalar::<i64>(
			&f,
			"SELECT count(*) FROM authorization_decisions WHERE tenant='acme' AND subject='carol' AND action='registration.read' AND NOT (decision->>'allowed')::boolean"
		)
		.await >= 1,
		"a denial survives its rolled-back transaction"
	);

	let groups = |subject: &str| format!("/api/tenants/acme/subjects/{subject}/groups");
	assert_eq!(
		admin
			.send(&app, reqwest::Method::PUT, &groups("root"))
			.json(&json!({"groups":[],"expected_policy_revision":revision}))
			.send()
			.await
			.unwrap()
			.status(),
		403,
		"a Tenant Administrator never changes its own subject"
	);
	let updated = admin
		.send(&app, reqwest::Method::PUT, &groups("carol"))
		.json(&json!({"groups":[],"expected_policy_revision":revision}))
		.send()
		.await
		.unwrap();
	assert_eq!(updated.status(), 200);
	let updated: Value = updated.json().await.unwrap();
	assert_eq!(updated["groups"], json!([]));
	assert_eq!(updated["policy_revision"], revision + 1);

	let mappings: Value = admin
		.get(&app, "/api/tenants/acme/mappings")
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	let mapping = |id: &Value| {
		mappings
			.as_array()
			.unwrap()
			.iter()
			.find(|row| row["id"] == *id)
			.unwrap()
			.clone()
	};
	let own = mapping(&admin_mapping["id"]);
	let theirs = mapping(&carol_mapping["id"]);
	// The dashboard hides the controls the server would reject from these.
	assert_eq!(
		(&own["own_identity"], &own["own_subject"]),
		(&json!(true), &json!(true))
	);
	assert_eq!(
		(&theirs["own_identity"], &theirs["own_subject"]),
		(&json!(false), &json!(false))
	);
	let disable = |row: &Value| {
		admin
			.send(
				&app,
				reqwest::Method::POST,
				&format!(
					"/api/tenants/acme/mappings/{}/disable",
					row["id"].as_str().unwrap()
				),
			)
			.json(&json!({"expected_revision":row["revision"]}))
			.send()
	};
	assert_eq!(disable(&own).await.unwrap().status(), 403);
	assert_eq!(disable(&theirs).await.unwrap().status(), 204);
	assert_eq!(
		carol
			.get(&app, "/api/session")
			.send()
			.await
			.unwrap()
			.status(),
		403
	);

	let history: Vec<(String, String, Option<uuid::Uuid>, Option<uuid::Uuid>)> = sqlx::query_as(
		"SELECT action, actor_kind, actor_identity_id, actor_mapping_id FROM dashboard_administration_history ORDER BY occurred_at, action",
	)
	.fetch_all(f.store.pool.fixture_driver())
	.await
	.unwrap();
	let admin_identity = uuid::Uuid::parse_str(admin_identity).unwrap();
	let admin_mapping = uuid::Uuid::parse_str(admin_mapping["id"].as_str().unwrap()).unwrap();
	assert_eq!(
		history,
		vec![
			(
				"registration.approve".into(),
				"deployment_administrator".into(),
				None,
				None
			),
			(
				"registration.approve".into(),
				"tenant_administrator".into(),
				Some(admin_identity),
				Some(admin_mapping)
			),
			// Clearing carol's groups changed the policy, so it is audited too.
			(
				"subject.group.update".into(),
				"tenant_administrator".into(),
				Some(admin_identity),
				Some(admin_mapping)
			),
			(
				"mapping.disable".into(),
				"tenant_administrator".into(),
				Some(admin_identity),
				Some(admin_mapping)
			),
		]
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn tenant_administrator_approval_evaluates_requested_groups(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	administered_tenants(&f).await;
	// `auditors` is assignable, but a conditional deny reserves it from every
	// administrator.
	let mut policy = administered_policy("acme", &["root"]);
	policy["groups"]["auditors"] = json!({"roles": ["member"], "assignable": true});
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id": "reserve-auditors",
		"effect": "deny",
		"subjects": {"groups": ["administrators"]},
		"actions": ["registration.approve"],
		"resources": {"kinds": ["registration"]},
		"condition": {
			"op": "contains",
			"left": {"source": "resource", "path": "/groups"},
			"right": {"source": "literal", "value": "auditors"}
		}
	}));
	replace_policy(&f, "acme", 1, policy).await;
	let app = common::application(f.clone()).await;
	let (admin, _) = mapped_browser(&app, &f, ("acme", "pool-a", "admin"), "root").await;
	let (cookie, csrf) = sign_in_as(&app, "pool-a", "newcomer", "Newcomer").await;
	let newcomer = register(&app, &cookie, &csrf).await;
	let approve = format!(
		"/api/tenants/acme/registrations/{}/approve",
		newcomer["id"].as_str().unwrap()
	);
	for (groups, status) in [
		(json!(["auditors"]), 403),
		(json!(["auditors", "members"]), 403),
		(json!(["members"]), 200),
	] {
		assert_eq!(
			admin
				.send(&app, reqwest::Method::POST, &approve)
				.json(&json!({"subject": "carol", "groups": groups}))
				.send()
				.await
				.unwrap()
				.status(),
			status,
			"{groups}"
		);
	}
	let snapshot: Value = browser()
		.get(app.url("/api/authorization/acme"))
		.bearer_auth(&f.config.api_token)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(
		snapshot["bundle"]["subjects"]["carol"]["groups"],
		json!(["members"])
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn tenant_administrator_sees_only_the_configured_issuers_bound_identities(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	administered_tenants(&f).await;
	let app = common::application(f.clone()).await;
	let (admin, admin_mapping) =
		mapped_browser(&app, &f, ("acme", "pool-a", "admin"), "root").await;
	// A previous GCIP project used the same GCIP Tenant ID.
	let previous = uuid::Uuid::new_v4();
	sqlx::query(
		"INSERT INTO dashboard_identities (id, issuer, subject, display_name) VALUES ($1, 'https://securetoken.google.com/previous-project/tenants/pool-a', 'person', 'Previous')",
	)
	.bind(previous)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	let registration = uuid::Uuid::new_v4();
	sqlx::query(
		"INSERT INTO dashboard_registration_requests (id, identity_id, status, created_at, expires_at) VALUES ($1, $2, 'pending', clock_timestamp(), clock_timestamp() + interval '7 days')",
	)
	.bind(registration)
	.bind(previous)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	// A full page of Mappings left by that issuer or by a previous Tenant
	// Binding sorts before every current Mapping.
	sqlx::query(
		"WITH identities AS (
			INSERT INTO dashboard_identities (id, issuer, subject)
			SELECT gen_random_uuid(),
				CASE WHEN n % 2 = 0 THEN 'https://securetoken.google.com/previous-project/tenants/pool-a' ELSE 'https://securetoken.google.com/fixture-project/tenants/pool-b' END,
				'stale-' || n
			FROM generate_series(1, 200) AS n
			RETURNING id, subject
		), credentials AS (
			INSERT INTO authorization_credentials (id, tenant, subject, token_hash, expires_at, issued_by)
			SELECT gen_random_uuid(), 'acme', 'a-' || subject, sha256(convert_to(subject, 'UTF8')), clock_timestamp() + interval '1 day', 'fixture'
			FROM identities
			RETURNING id, subject
		)
		INSERT INTO dashboard_mappings (id, identity_id, tenant, subject, credential_id)
		SELECT gen_random_uuid(), i.id, 'acme', c.subject, c.id
		FROM identities i JOIN credentials c ON c.subject = 'a-' || i.subject",
	)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	// Another GCIP Tenant's queue, larger than one statement's bind parameter
	// limit, is never read by this Tenant's list.
	sqlx::query(
		"WITH identities AS (
			INSERT INTO dashboard_identities (id, issuer, subject)
			SELECT gen_random_uuid(), 'https://securetoken.google.com/fixture-project/tenants/pool-b', 'queued-' || n
			FROM generate_series(1, 70000) AS n
			RETURNING id
		)
		INSERT INTO dashboard_registration_requests (id, identity_id, status, created_at, expires_at)
		SELECT gen_random_uuid(), id, 'pending', clock_timestamp(), clock_timestamp() + interval '7 days'
		FROM identities",
	)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();

	let listed: Value = admin
		.get(&app, "/api/tenants/acme/registrations")
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(listed, json!([]));
	let decide = |action: &str| {
		admin
			.send(
				&app,
				reqwest::Method::POST,
				&format!("/api/tenants/acme/registrations/{registration}/{action}"),
			)
			.json(&json!({"subject": "mallory", "groups": ["members"]}))
			.send()
	};
	assert_eq!(decide("reject").await.unwrap().status(), 404);
	assert_eq!(decide("approve").await.unwrap().status(), 404);
	let mappings: Value = admin
		.get(&app, "/api/tenants/acme/mappings?offset=0")
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	let ids: Vec<&Value> = mappings
		.as_array()
		.unwrap()
		.iter()
		.map(|row| &row["id"])
		.collect();
	assert_eq!(ids, vec![&admin_mapping["id"]], "{mappings}");
	common::cleanup(f, &url, &schema).await;
}

/// Waits until `count` backends of this test's database wait on a lock.
async fn lock_waiters(f: &Federation, count: i64) {
	tokio::time::timeout(std::time::Duration::from_secs(10), async {
		while scalar::<i64>(
			f,
			"SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock'",
		)
		.await != count
		{
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	})
	.await
	.unwrap_or_else(|_| panic!("{count} backends never waited on a lock"));
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tenant_administrators_disabling_each_other_are_serialized(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	administered_tenants(&f).await;
	replace_policy(
		&f,
		"acme",
		1,
		administered_policy("acme", &["root", "deputy"]),
	)
	.await;
	let app = common::application(f.clone()).await;
	let (first, first_mapping) =
		mapped_browser(&app, &f, ("acme", "pool-a", "first"), "root").await;
	let (second, second_mapping) =
		mapped_browser(&app, &f, ("acme", "pool-a", "second"), "deputy").await;
	// Each disable's authorization decision waits on the decision log's
	// serialization lock, holding every lock it took before the decision.
	let mut barrier = f.store.pool.fixture_driver().begin().await.unwrap();
	sqlx::query("SELECT pg_advisory_xact_lock(71003202)")
		.execute(&mut *barrier)
		.await
		.unwrap();
	let disable = |actor: &Browser, target: &Value| {
		tokio::spawn(
			actor
				.send(
					&app,
					reqwest::Method::POST,
					&format!(
						"/api/tenants/acme/mappings/{}/disable",
						target["id"].as_str().unwrap()
					),
				)
				.json(&json!({"expected_revision": 1}))
				.send(),
		)
	};
	let first_disables = disable(&first, &second_mapping);
	lock_waiters(&f, 1).await;
	let second_disables = disable(&second, &first_mapping);
	lock_waiters(&f, 2).await;
	barrier.commit().await.unwrap();
	assert_eq!(first_disables.await.unwrap().unwrap().status(), 204);
	// The second administrator's own Mapping was disabled before it acted.
	assert_eq!(second_disables.await.unwrap().unwrap().status(), 401);
	let enabled = |mapping: &Value| {
		format!(
			"SELECT enabled FROM dashboard_mappings WHERE id = '{}'",
			mapping["id"].as_str().unwrap()
		)
	};
	assert!(scalar::<bool>(&f, &enabled(&first_mapping)).await);
	assert!(!scalar::<bool>(&f, &enabled(&second_mapping)).await);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn the_mfa_requirement_is_only_a_sign_in_hint_and_second_factor_claims_are_not_authority(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	let app = common::application(f.clone()).await;
	// The transaction tells the sign-in page which pools must enroll a factor.
	for (pool, mandatory) in [("pool-a", false), ("pool-b", true)] {
		let (state, cookie) = transaction(&app, pool).await;
		let configuration: Value = browser()
			.get(app.url(format!("/auth/gcip/transaction?state={state}")))
			.header("cookie", &cookie)
			.send()
			.await
			.unwrap()
			.json()
			.await
			.unwrap();
		assert_eq!(configuration["mfa_mandatory"], mandatory, "{pool}");
	}
	// Presence or absence of the second-factor claims maps to the same authority.
	let now = Utc::now().timestamp();
	let mut sessions = Vec::new();
	for token in [
		signed_token("pool-a", "person", "Person", now, "password"),
		second_factor_token("pool-a", "person", "Person", now),
	] {
		let (state, cookie) = transaction(&app, "pool-a").await;
		let response = exchange(&app, &state, &cookie, &token, "https://127.0.0.1:8080").await;
		assert_eq!(response.status(), 200);
		let session = response
			.headers()
			.get_all("set-cookie")
			.iter()
			.next()
			.unwrap();
		let session = session
			.to_str()
			.unwrap()
			.split(';')
			.next()
			.unwrap()
			.to_owned();
		let info = browser()
			.get(app.url("/auth/session"))
			.header("cookie", &session)
			.send()
			.await
			.unwrap();
		assert_eq!(info.status(), 200);
		sessions.push(info.json::<Value>().await.unwrap());
	}
	assert_eq!(
		sessions[0]["deployment_administrator"],
		sessions[1]["deployment_administrator"]
	);
	assert_eq!(sessions[0]["mappings"], sessions[1]["mappings"]);
	assert_ne!(sessions[0]["id"], sessions[1]["id"]);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn signing_in_again_from_a_session_reenters_through_its_own_gcip_tenant(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	let app = common::application(f.clone()).await;
	let (session, _) = sign_in(&app, "pool-b", "Person").await;
	// Act: second-factor enrollment starts this way, with no Tenant name.
	let response = browser()
		.get(app.url("/auth/login?return_to=%2Fsettings"))
		.header("cookie", &session)
		.send()
		.await
		.unwrap();
	// Assert
	assert_eq!(response.status(), 303);
	let state = response.headers()["location"]
		.to_str()
		.unwrap()
		.strip_prefix("/sign-in?state=")
		.unwrap()
		.to_owned();
	let cookie = response.headers()["set-cookie"]
		.to_str()
		.unwrap()
		.split(';')
		.next()
		.unwrap()
		.to_owned();
	let client: Value = browser()
		.get(app.url(format!("/auth/gcip/transaction?state={state}")))
		.header("cookie", &cookie)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(client["tenant_id"], "pool-b");
	let now = Utc::now().timestamp();
	let other = token("pool-a", "Person", now);
	assert_eq!(
		exchange(&app, &state, &cookie, &other, "https://127.0.0.1:8080")
			.await
			.status(),
		401
	);
	let same = token("pool-b", "Person", now);
	assert_eq!(
		exchange(&app, &state, &cookie, &same, "https://127.0.0.1:8080")
			.await
			.status(),
		200
	);
	// Without a session the same request only shows the sign-in page.
	let anonymous = browser()
		.get(app.url("/auth/login?return_to=%2Fsettings"))
		.send()
		.await
		.unwrap();
	assert_eq!(
		anonymous.headers()["location"],
		"/sign-in?return_to=%2Fsettings"
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn removing_the_gcip_sign_in_refuses_its_identities_durable_credentials(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	let policy = serde_json::from_value(json!({
		"tenant": "acme",
		"subjects": {"alice": {"kind": "user"}},
		"policies": [{"id": "alice", "effect": "allow",
			"subjects": {"ids": ["alice"]}, "actions": ["*"],
			"resources": {"kinds": ["*"], "ids": []}}]
	}))
	.unwrap();
	common::tenant_root_in(f.store.pool.fixture_driver(), "acme").await;
	aidash_server::authorization::Authorization {
		pool: f.store.pool.clone(),
	}
	.replace("acme", 0, policy, "deployment_administrator")
	.await
	.unwrap();
	let app = common::application(f.clone()).await;
	let (member, _) = mapped_browser(&app, &f, ("acme", "pool-a", "member"), "alice").await;
	// A member's own credential, which outlives every browser session.
	let issued = browser()
		.post(app.url("/api/authorization/acme/credentials"))
		.header("cookie", &member.cookie)
		.header("origin", "https://127.0.0.1:8080")
		.header("x-aidash-csrf", &member.csrf)
		.header("x-aidash-context", &member.context)
		.json(&json!({"subject": "alice", "expires_in_seconds": 3600}))
		.send()
		.await
		.unwrap();
	assert_eq!(issued.status(), 200);
	let token = issued.json::<Value>().await.unwrap()["token"]
		.as_str()
		.unwrap()
		.to_owned();
	// A Tenant boundary, which locks the credential's Mapping and its holder.
	let session = |app: &common::TestApplication| {
		browser()
			.get(app.url("/api/registry/versions"))
			.bearer_auth(&token)
			.send()
	};
	assert_eq!(session(&app).await.unwrap().status(), 200);
	// Act: the Deployment drops [dashboard.gcip] and keeps its database.
	let mut unconfigured = f.clone();
	unconfigured.config.sign_in = None;
	unconfigured.store = unconfigured
		.store
		.clone()
		.with_dashboard_policy(unconfigured.config.dashboard_policy());
	let restarted = common::application(unconfigured.clone()).await;
	// Assert
	assert_eq!(session(&restarted).await.unwrap().status(), 403);
	let disabled: Option<chrono::DateTime<Utc>> = scalar(
		&f,
		"SELECT disabled_at FROM dashboard_identities WHERE subject = 'member'",
	)
	.await;
	assert!(disabled.is_some());
	common::cleanup(f, &url, &schema).await;
}

/// Make `pool-s` the Shared Pool beside the fixture's bound pools.
fn share(f: &mut Federation) {
	reconfigure(f, |gcip| gcip.shared_pool = Some("pool-s".into()));
}

async fn identity_disabled(f: &Federation, uid: &str) -> bool {
	scalar::<Option<chrono::DateTime<Utc>>>(
		f,
		&format!("SELECT disabled_at FROM dashboard_identities WHERE subject = '{uid}'"),
	)
	.await
	.is_some()
}

#[rstest::rstest]
#[tokio::test]
async fn the_shared_pool_signs_in_every_unrouted_address_under_the_tenancy_rules(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (mut f, url, schema) = runtime.parts();
	f.config.tenancy.personal_tenants = true;
	share(&mut f);
	let app = common::application(f.clone()).await;
	// An address in no Sign-in Domain, and a malformed one, go to the Shared Pool.
	for body in [
		"email=alice%40unmapped.example&return_to=%2Fsettings",
		"email=not-an-email&return_to=%2Fsettings",
	] {
		let response = discover(&app, body, Some("https://127.0.0.1:8080")).await;
		assert_eq!(response.status(), 303, "{body}");
		let state = response.headers()["location"]
			.to_str()
			.unwrap()
			.strip_prefix("/sign-in?state=")
			.unwrap()
			.to_owned();
		let cookie = response.headers()["set-cookie"]
			.to_str()
			.unwrap()
			.split(';')
			.next()
			.unwrap()
			.to_owned();
		let client: Value = browser()
			.get(app.url(format!("/auth/gcip/transaction?state={state}")))
			.header("cookie", &cookie)
			.send()
			.await
			.unwrap()
			.json()
			.await
			.unwrap();
		assert_eq!(client["tenant_id"], "pool-s", "{body}");
	}
	// The Tenancy Rules give a Shared Pool identity its Personal Tenant, and
	// never a bound one.
	let mappings = |cookie: String| {
		let app = &app;
		async move {
			browser()
				.get(app.url("/auth/session"))
				.header("cookie", cookie)
				.send()
				.await
				.unwrap()
				.json::<Value>()
				.await
				.unwrap()["mappings"]
				.clone()
		}
	};
	let (shared, _) = sign_in_as(&app, "pool-s", "shared", "Shared").await;
	let personal = mappings(shared).await;
	assert_eq!(personal.as_array().unwrap().len(), 1, "{personal}");
	assert_eq!(personal[0]["personal"], true, "{personal}");
	let (bound, _) = sign_in_as(&app, "pool-a", "bound", "Bound").await;
	assert_eq!(mappings(bound).await, json!([]));
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn an_identity_whose_pool_changes_between_shared_and_bound_waits_for_a_restore(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (mut f, url, schema) = runtime.parts();
	share(&mut f);
	let app = common::application(f.clone()).await;
	sign_in_as(&app, "pool-s", "moving", "Moving").await;
	sign_in_as(&app, "pool-s", "starting", "Starting").await;
	common::tenant_root(&f, "third").await;
	// Act: the Shared Pool gets a Tenant Binding.
	reconfigure(&mut f, |gcip| {
		gcip.shared_pool = None;
		gcip.tenant_bindings.insert("pool-s".into(), "third".into());
		gcip.sign_in_domains
			.insert("third.example".into(), "pool-s".into());
	});
	let bound = common::application(f.clone()).await;
	// Assert: startup disables an identity that has not signed in since...
	f.config
		.sign_in
		.as_ref()
		.unwrap()
		.reconcile(&f)
		.await
		.unwrap();
	assert!(identity_disabled(&f, "starting").await);
	// ...and a sign-in refuses and disables one it has not seen.
	sqlx::query("UPDATE dashboard_identities SET disabled_at = NULL WHERE subject = 'moving'")
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	sqlx::query("UPDATE gcip_admissions SET tenant = NULL WHERE identity_id = (SELECT id FROM dashboard_identities WHERE subject = 'moving')")
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	let (state, cookie) = transaction(&bound, "pool-s").await;
	let now = Utc::now().timestamp();
	assert_eq!(
		exchange(
			&bound,
			&state,
			&cookie,
			&signed_token("pool-s", "moving", "Moving", now, "password"),
			"https://127.0.0.1:8080"
		)
		.await
		.status(),
		403
	);
	assert!(identity_disabled(&f, "moving").await);
	let (state, cookie) = transaction(&bound, "pool-s").await;
	assert_eq!(
		exchange(
			&bound,
			&state,
			&cookie,
			&signed_token("pool-s", "moving", "Moving", now, "password"),
			"https://127.0.0.1:8080"
		)
		.await
		.status(),
		403,
		"a second sign-in before the restore stays refused"
	);
	// Only a Deployment Administrator restore admits it again, as bound.
	let id: uuid::Uuid = scalar(
		&f,
		"SELECT id FROM dashboard_identities WHERE subject = 'moving'",
	)
	.await;
	let restored = browser()
		.post(bound.url(format!("/api/dashboard/identities/{id}/restore")))
		.bearer_auth(&f.config.api_token)
		.send()
		.await
		.unwrap();
	assert_eq!(restored.status(), 204);
	sign_in_as(&bound, "pool-s", "moving", "Moving").await;
	let recorded: Option<String> = scalar(
		&f,
		&format!("SELECT tenant FROM gcip_admissions WHERE identity_id = '{id}'"),
	)
	.await;
	assert_eq!(recorded.as_deref(), Some("third"));
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_gcip_tenant_neither_shared_nor_bound_admits_nobody(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (mut f, url, schema) = runtime.parts();
	share(&mut f);
	let app = common::application(f.clone()).await;
	let (state, cookie) = transaction(&app, "pool-s").await;
	sign_in_as(&app, "pool-s", "shared", "Shared").await;
	// Act: the Deployment stops naming a Shared Pool.
	reconfigure(&mut f, |gcip| gcip.shared_pool = None);
	let unshared = common::application(f.clone()).await;
	// Assert
	assert_eq!(
		discover(
			&unshared,
			"email=alice%40unmapped.example",
			Some("https://127.0.0.1:8080")
		)
		.await
		.headers()["location"],
		"/sign-in?error=not_found&return_to=%2F"
	);
	assert_eq!(
		exchange(
			&unshared,
			&state,
			&cookie,
			&signed_token(
				"pool-s",
				"shared",
				"Shared",
				Utc::now().timestamp(),
				"password"
			),
			"https://127.0.0.1:8080"
		)
		.await
		.status(),
		403
	);
	assert!(identity_disabled(&f, "shared").await);
	common::cleanup(f, &url, &schema).await;
}

/// Run startup with these GCIP settings, as a Cloud Environment's managed file
/// supplies them.
async fn start(
	f: &Federation,
	url: &str,
	gcip: &GcipConfig,
) -> aidash_server::Result<aidash_server::federation::Federation> {
	let mut settings = common::settings_for(url);
	settings.node.node_id = f.config.node_id.clone();
	settings.node.endpoint = f.config.endpoint.clone();
	settings.node.api_token = f.config.api_token.clone();
	let managed = tempfile::NamedTempFile::new().unwrap();
	std::fs::write(
		managed.path(),
		serde_json::to_vec(&json!({"dashboard": {"gcip": gcip}})).unwrap(),
	)
	.unwrap();
	// SAFETY: no other test of this binary reads the variable.
	unsafe { std::env::set_var("AIDASH_GCIP_SETTINGS", managed.path()) };
	let context = reinhardt::test::fixtures::injection_context::default();
	let started =
		aidash_server::bootstrap::initialize(&context, &settings, f.store.pool.connection()).await;
	unsafe { std::env::remove_var("AIDASH_GCIP_SETTINGS") };
	if let Ok(started) = &started {
		started.store.control_pool.close().await;
	}
	started
}

#[rstest::rstest]
#[tokio::test]
async fn startup_refuses_a_binding_to_a_missing_or_personal_tenant_and_keeps_a_deleted_one(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (f, url, schema) = runtime.parts();
	let bind = |tenant: &str| {
		let mut gcip = gcip_settings();
		gcip.tenant_bindings.insert("pool-b".into(), tenant.into());
		gcip
	};
	let refusal = start(&f, &url, &bind("missing")).await.err().unwrap();
	assert_eq!(
		refusal.to_string(),
		"a sign-in restricts identities to Tenant missing, which has no root record"
	);
	common::tenant_root(&f, "solo").await;
	sqlx::query("UPDATE tenants SET personal = true WHERE name = 'solo'")
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	let refusal = start(&f, &url, &bind("solo")).await.err().unwrap();
	assert_eq!(
		refusal.to_string(),
		"a sign-in restricts identities to Tenant solo, which is a Personal Tenant"
	);
	// A deleted bound Tenant keeps its root record: startup proceeds, and
	// sign-in through its GCIP Tenant gets the deleted Tenant's answer.
	sqlx::query(
		"INSERT INTO tenant_deletions (id, tenant, deleted_at, deleted_by_kind, purge_after, purge_failures) VALUES (gen_random_uuid(), 'other', clock_timestamp(), 'deployment_administrator', clock_timestamp() + interval '30 days', 0)",
	)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	start(&f, &url, &gcip_settings()).await.unwrap();
	let app = common::application(f.clone()).await;
	let (state, cookie) = transaction(&app, "pool-b").await;
	let refused = exchange(
		&app,
		&state,
		&cookie,
		&token("pool-b", "Person", Utc::now().timestamp()),
		"https://127.0.0.1:8080",
	)
	.await;
	assert_eq!(refused.status(), 410);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_bound_tenants_administrators_decide_a_shared_pool_identitys_invitation(
	#[future(awt)]
	#[from(gcip_runtime)]
	gcip: GcipFixture,
) {
	let GcipFixture {
		runtime, _admin, ..
	} = gcip;
	let (mut f, url, schema) = runtime.parts();
	administered_tenants(&f).await;
	share(&mut f);
	let app = common::application(f.clone()).await;
	let (admin, _) = mapped_browser(&app, &f, ("acme", "pool-a", "admin"), "root").await;
	let invitation: Value = admin
		.send(&app, reqwest::Method::POST, "/api/tenants/acme/invitations")
		.json(&json!({}))
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	let (cookie, csrf) = sign_in_as(&app, "pool-s", "guest", "Guest").await;
	let redeemed = browser()
		.post(app.url("/auth/invitations/redeem"))
		.header("cookie", &cookie)
		.header("origin", "https://127.0.0.1:8080")
		.header("x-aidash-csrf", &csrf)
		.json(&json!({"tenant": "acme", "secret": invitation["secret"]}))
		.send()
		.await
		.unwrap();
	assert_eq!(redeemed.status(), 200);
	let request: Value = redeemed.json().await.unwrap();
	// Assert: the bound Tenant's administrators see and decide it.
	let listed: Value = admin
		.get(&app, "/api/tenants/acme/registrations")
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
	assert_eq!(listed[0]["id"], request["id"]);
	let approved = admin
		.send(
			&app,
			reqwest::Method::POST,
			&format!(
				"/api/tenants/acme/registrations/{}/approve",
				request["id"].as_str().unwrap()
			),
		)
		.json(&json!({"subject": "guest", "groups": ["members"]}))
		.send()
		.await
		.unwrap();
	assert_eq!(approved.status(), 200);
	common::cleanup(f, &url, &schema).await;
}
