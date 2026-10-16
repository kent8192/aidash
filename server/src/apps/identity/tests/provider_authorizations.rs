//! Provider Authorization through the production router and PostgreSQL. The
//! in-process fake OpenRouter enforces PKCE, single-use codes and code expiry;
//! the HTTP adapter itself is exercised against a fake endpoint in
//! `aidash-integrations`.
#[path = "../../execution/tests/support/legacy.rs"]
mod common;
use aidash_application::{
	Error, Result,
	provider_authorization::Exchange,
	provider_credentials::{KeyValidator, Service, Store, Validation},
};
use aidash_domain::provider_credentials::Provider;
use aidash_server::{
	apps::identity::repositories::provider_credentials::NativeRepository,
	authorization::{Authorization, policy::PolicyBundle},
	config::OidcConfig,
	federation::Federation,
};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::FutureExt;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use reinhardt::test::TestResponse;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
	collections::BTreeMap,
	sync::{
		Arc, Mutex,
		atomic::{AtomicBool, Ordering},
	},
};
use uuid::Uuid;

const ORIGIN: &str = "http://127.0.0.1:8080";
const CALLBACK: &str = "/auth/provider-authorizations/openrouter/callback";

/// Issued codes by value: the S256 challenge they are bound to, whether they
/// have expired, and the Key Material they exchange for.
type IssuedCodes = BTreeMap<String, (String, bool, String)>;
#[derive(Clone, Default)]
struct FakeOpenRouter {
	codes: Arc<Mutex<IssuedCodes>>,
}
impl FakeOpenRouter {
	/// What the provider does after consent: bind a fresh code to the challenge.
	fn consent(&self, challenge: &str, expired: bool) -> (String, String) {
		let code = format!("code-{}", Uuid::new_v4().simple());
		let key = format!("sk-or-v1-{}", Uuid::new_v4().simple());
		self.codes
			.lock()
			.unwrap()
			.insert(code.clone(), (challenge.into(), expired, key.clone()));
		(code, key)
	}
}
#[async_trait]
impl Exchange for FakeOpenRouter {
	async fn exchange(
		&self,
		_: Provider,
		code: &str,
		verifier: &SecretString,
	) -> Result<SecretString> {
		let rejected = || Error::Invalid("Provider Authorization was rejected".into());
		// Codes are single-use whether or not the exchange succeeds.
		let (challenge, expired, key) = self
			.codes
			.lock()
			.unwrap()
			.remove(code)
			.ok_or_else(rejected)?;
		if expired
			|| URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.expose_secret().as_bytes()))
				!= challenge
		{
			return Err(rejected());
		}
		Ok(key.into())
	}
}

#[derive(Default)]
struct MemoryStore {
	versions: Mutex<BTreeMap<String, Vec<String>>>,
}
#[async_trait]
impl Store for MemoryStore {
	fn resource(&self, id: Uuid) -> String {
		format!("memory/{id}")
	}
	async fn create(&self, _: &str, id: Uuid) -> Result<()> {
		self.versions
			.lock()
			.unwrap()
			.insert(self.resource(id), vec![]);
		Ok(())
	}
	async fn add_version(&self, _: &str, resource: &str, key: &SecretString) -> Result<String> {
		let mut versions = self.versions.lock().unwrap();
		let entry = versions.get_mut(resource).unwrap();
		// Retain the fixture key so the test can prove which key was stored.
		entry.push(key.expose_secret().to_owned());
		Ok(format!("{resource}/versions/{}", entry.len()))
	}
	async fn versions(&self, resource: &str) -> Result<Vec<String>> {
		Ok((1..=self
			.versions
			.lock()
			.unwrap()
			.get(resource)
			.map_or(0, Vec::len))
			.map(|n| format!("{resource}/versions/{n}"))
			.collect())
	}
	async fn disable(&self, _: &str) -> Result<()> {
		Ok(())
	}
	async fn destroy(&self, _: &str) -> Result<()> {
		Ok(())
	}
	async fn delete(&self, resource: &str) -> Result<()> {
		self.versions.lock().unwrap().remove(resource);
		Ok(())
	}
}
/// Reports a missing spending limit, as OpenRouter's validation does, while
/// `uncapped` is set.
#[derive(Clone, Default)]
struct Validator {
	uncapped: Arc<AtomicBool>,
}
#[async_trait]
impl KeyValidator for Validator {
	async fn validate(&self, _: Provider, _: &SecretString) -> Result<Validation> {
		Ok(Validation {
			warnings: if self.uncapped.load(Ordering::SeqCst) {
				vec!["No spending limit is configured for this Provider Credential".into()]
			} else {
				vec![]
			},
		})
	}
}

/// The fake provider, validator and Store the test observes; shared with the
/// runtime.
#[derive(Clone, Default)]
struct Parts {
	provider: FakeOpenRouter,
	validator: Validator,
	store: Arc<MemoryStore>,
}
#[rstest::fixture]
fn parts() -> Parts {
	Parts::default()
}

async fn insert_session(f: &Federation, identity: Uuid, token: &str, csrf: &str) {
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
		.from_subquery(
			Query::select()
				.expr(Expr::val(Uuid::new_v4()))
				.expr(Expr::val(identity))
				.expr(Expr::val(Sha256::digest(token.as_bytes()).to_vec()))
				.expr(Expr::val(Sha256::digest(csrf.as_bytes()).to_vec()))
				.expr(Expr::cust("clock_timestamp()"))
				.expr(Expr::cust("clock_timestamp()"))
				.expr(Expr::cust("clock_timestamp()+interval '12 hours'"))
				.to_owned(),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&query)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
}

/// A browser identity with a Deployment Administrator grant and two dashboard sessions, and
/// a Provider Credential Store whose exchange is the fake OpenRouter.
#[rstest::fixture]
fn authorization_runtime(
	#[default(Parts::default())] parts: Parts,
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) -> common::RuntimeFuture {
	async move {
		let mut owner = runtime.await;
		let f = &mut owner.federation;
		f.config.oidc = Some(OidcConfig {
			issuer: "https://accounts.google.com".into(),
			client_id: "fixture".into(),
			client_secret: "server-only-fixture".into(),
			public_origin: ORIGIN.into(),
			keycloak_admin_url: String::new(),
			status_client_id: String::new(),
			status_client_secret: String::new(),
			session_absolute_seconds: 43_200,
			session_idle_seconds: 1_800,
		});
		let identity = Uuid::new_v4();
		for query in [
			Query::insert()
				.into_table(Alias::new("dashboard_identities"))
				.columns(["id", "issuer", "subject", "last_valid_at"].map(Alias::new))
				.from_subquery(
					Query::select()
						.expr(Expr::val(identity))
						.expr(Expr::val("https://accounts.google.com"))
						.expr(Expr::val("provider-authorization-fixture"))
						.expr(Expr::cust("clock_timestamp()"))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
			Query::insert()
				.into_table(Alias::new("dashboard_deployment_administrator_grants"))
				.columns([Alias::new("identity_id")])
				.from_subquery(Query::select().expr(Expr::val(identity)).to_owned())
				.to_string(PostgresQueryBuilder),
		] {
			sqlx::query(&query)
				.execute(f.store.pool.driver())
				.await
				.unwrap();
		}
		insert_session(f, identity, "browser-a", "csrf-a").await;
		insert_session(f, identity, "browser-b", "csrf-b").await;
		let policy: PolicyBundle = serde_json::from_value(json!({
			"tenant":"alpha","subjects":{"alice":{"kind":"user"}},"policies":[]
		}))
		.unwrap();
		Authorization {
			pool: f.store.pool.clone(),
		}
		.replace("alpha", 0, policy, "deployment_administrator")
		.await
		.unwrap();
		f.store.provider_credentials = Some(Arc::new(Service {
			repository: Arc::new(NativeRepository {
				pool: f.store.control_pool.clone(),
				node: f.store.node_id.clone(),
			}),
			store: parts.store.clone(),
			validator: Arc::new(parts.validator.clone()),
			exchange: Arc::new(parts.provider.clone()),
			fingerprint_key: "test-fingerprint-key-that-is-at-least-32-bytes".into(),
			max_per_tenant: 5,
		}));
		owner
	}
	.boxed()
	.shared()
}
#[rstest::fixture]
fn authorization_application(
	#[default(Parts::default())] _parts: Parts,
	#[from(authorization_runtime)]
	#[with(_parts.clone())]
	_runtime: common::RuntimeFuture,
	#[from(common::direct_application)]
	#[with(aidash_server::http::Settings { auth_burst: 10000, ..Default::default() }, aidash_server::sse::Service::new(Default::default()), std::sync::Arc::new(|r| r), _runtime.clone())]
	application: common::ApplicationFuture,
) -> common::ApplicationFuture {
	application
}

struct Fixture {
	f: Federation,
	app: common::TestApplication,
	provider: FakeOpenRouter,
	validator: Validator,
	store: Arc<MemoryStore>,
	url: String,
	schema: String,
}
impl Fixture {
	fn new(application: common::ApplicationFixture, parts: Parts) -> Self {
		let (f, url, schema) = application.runtime.parts();
		Self {
			f,
			app: application.application,
			provider: parts.provider,
			validator: parts.validator,
			store: parts.store,
			url,
			schema,
		}
	}
}

async fn send(
	app: &common::TestApplication,
	method: &str,
	path: &str,
	headers: &[(&str, &str)],
	body: Value,
) -> TestResponse {
	let body = if body.is_null() {
		Vec::new()
	} else {
		body.to_string().into_bytes()
	};
	common::http_response(app, method, path, headers, &body).await
}
fn cookie(response: &TestResponse, name: &str) -> Option<String> {
	response
		.headers()
		.get_all("set-cookie")
		.iter()
		.filter_map(|value| value.to_str().ok())
		.find_map(|value| {
			let (pair, _) = value.split_once(';')?;
			let (key, value) = pair.split_once('=')?;
			(key == name).then(|| value.to_owned())
		})
}

/// One started authorization: its browser binding and the consent URL query.
struct Started {
	browser: String,
	state: String,
	challenge: String,
}
async fn start(fixture: &Fixture, session: &str, body: Value, browser: Option<&str>) -> Started {
	start_as(fixture, session, "deployment_administrator", body, browser).await
}
/// The browser request that starts an authorization under a dashboard context.
async fn start_request(
	fixture: &Fixture,
	session: &str,
	context: &str,
	body: Value,
	browser: Option<&str>,
) -> TestResponse {
	let csrf = if session == "browser-a" {
		"csrf-a"
	} else {
		"csrf-b"
	};
	let cookies = match browser {
		Some(browser) => {
			format!("aidash-session={session}; aidash-provider-authorization={browser}")
		}
		None => format!("aidash-session={session}"),
	};
	send(
		&fixture.app,
		"POST",
		"/api/tenants/alpha/provider-authorizations/openrouter",
		&[
			("content-type", "application/json"),
			("cookie", &cookies),
			("origin", ORIGIN),
			("x-aidash-csrf", csrf),
			("x-aidash-context", context),
		],
		body,
	)
	.await
}
async fn start_as(
	fixture: &Fixture,
	session: &str,
	context: &str,
	body: Value,
	browser: Option<&str>,
) -> Started {
	let response = start_request(fixture, session, context, body, browser).await;
	let bytes = response.body();
	assert_eq!(response.status(), 200, "{}", String::from_utf8_lossy(bytes));
	assert_eq!(response.headers()["cache-control"], "no-store");
	let browser =
		cookie(&response, "aidash-provider-authorization").expect("start binds the browser");
	let started: Value = serde_json::from_slice(bytes).unwrap();
	let url = reqwest::Url::parse(started["authorization_url"].as_str().unwrap()).unwrap();
	assert_eq!(url.origin().ascii_serialization(), "https://openrouter.ai");
	let query: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
	assert_eq!(query["callback_url"], format!("{ORIGIN}{CALLBACK}"));
	assert_eq!(query["key_label"], "Aidash · alpha");
	assert_eq!(query["code_challenge_method"], "S256");
	Started {
		browser,
		state: query["state"].clone(),
		challenge: query["code_challenge"].clone(),
	}
}
async fn callback(fixture: &Fixture, session: &str, browser: &str, query: &str) -> TestResponse {
	send(
		&fixture.app,
		"GET",
		&format!("{CALLBACK}?{query}"),
		&[(
			"cookie",
			&format!("aidash-session={session}; aidash-provider-authorization={browser}"),
		)],
		Value::Null,
	)
	.await
}
fn location(response: &TestResponse) -> &str {
	response.headers()["location"].to_str().unwrap()
}
async fn credentials(fixture: &Fixture) -> Vec<Value> {
	let (status, rows) = common::request(
		&fixture.app,
		&fixture.f.config.api_token,
		"GET",
		"/api/tenants/alpha/provider-credentials",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{rows}");
	serde_json::from_value(rows).unwrap()
}
fn stored_versions(fixture: &Fixture) -> usize {
	fixture
		.store
		.versions
		.lock()
		.unwrap()
		.values()
		.map(Vec::len)
		.sum()
}

#[rstest::rstest]
#[tokio::test]
async fn connect_and_reconnect_write_only_through_oauth_and_never_disclose_keys(
	#[from(parts)] parts: Parts,
	#[future(awt)]
	#[from(authorization_application)]
	#[with(parts.clone())]
	application: common::ApplicationFixture,
) {
	let fixture = Fixture::new(application, parts);
	let started = start(&fixture, "browser-a", json!({}), None).await;
	let (code, key) = fixture.provider.consent(&started.challenge, false);
	let response = callback(
		&fixture,
		"browser-a",
		&started.browser,
		&format!("state={}&code={code}", started.state),
	)
	.await;
	assert_eq!(response.status(), 303);
	assert_eq!(
		location(&response),
		"/settings?view=providerCredentials&provider_authorization=connected"
	);
	let rows = credentials(&fixture).await;
	assert_eq!(rows.len(), 1);
	let row = &rows[0];
	assert_eq!(row["state"], "active");
	let digest: String = Sha256::digest(key.as_bytes())
		.iter()
		.map(|b| format!("{b:02x}"))
		.collect();
	assert_eq!(
		row["key_settings_url"],
		format!("https://openrouter.ai/keys/{digest}")
	);
	assert!(!row.to_string().contains(&key));
	assert_eq!(
		fixture.store.versions.lock().unwrap()[&format!("memory/{}", row["id"].as_str().unwrap())],
		vec![key.clone()]
	);
	// A first connection binds the Tenant's unbound provider.
	assert_eq!(bound_credential(&fixture).await, row["id"]);

	// Reconnect rotates the same record to a newly issued key. The replaced key
	// stays live at OpenRouter, so the redirect carries its settings page, and
	// OpenRouter's missing-spending-limit warning reaches the dashboard.
	fixture.validator.uncapped.store(true, Ordering::SeqCst);
	let started = start(
		&fixture,
		"browser-a",
		json!({"provider_credential_id": row["id"], "expected_revision": row["revision"], "return_to": "/settings"}),
		Some(&started.browser),
	)
	.await;
	let (code, rotated_key) = fixture.provider.consent(&started.challenge, false);
	let response = callback(
		&fixture,
		"browser-a",
		&started.browser,
		&format!("state={}&code={code}", started.state),
	)
	.await;
	assert_eq!(
		location(&response),
		format!(
			"/settings?provider_authorization=reconnected&provider_key_settings_url=https%3A%2F%2Fopenrouter%2Eai%2Fkeys%2F{digest}&provider_key_warnings=1"
		)
	);
	fixture.validator.uncapped.store(false, Ordering::SeqCst);
	let rows = credentials(&fixture).await;
	assert_eq!(rows.len(), 1);
	assert!(rows[0]["rotated_at"].is_string());
	assert_eq!(stored_versions(&fixture), 2);
	// Key Material never reaches PostgreSQL in any table.
	let mut dump = String::new();
	for table in ["provider_authorizations", "provider_credentials", "events"] {
		let query = Query::select()
			.expr_as(
				Expr::col(Alias::new("record")).cast_as(Alias::new("text")),
				Alias::new("document"),
			)
			.from_as(Alias::new(table), Alias::new("record"))
			.to_string(PostgresQueryBuilder);
		let text: Vec<String> = sqlx::query_scalar(&query)
			.fetch_all(fixture.f.store.pool.driver())
			.await
			.unwrap();
		let text = text.concat();
		dump.push_str(&text);
	}
	let rotated_digest: String = Sha256::digest(rotated_key.as_bytes())
		.iter()
		.map(|b| format!("{b:02x}"))
		.collect();
	assert!(dump.contains(&rotated_digest) && !dump.contains(&digest));
	assert!(!dump.contains(&key) && !dump.contains(&rotated_key));

	// A Connect that loses to an active binding (here: another connection that
	// is already bound) stores its key but never reports `connected`.
	let started = start(&fixture, "browser-a", json!({}), Some(&started.browser)).await;
	let (code, _) = fixture.provider.consent(&started.challenge, false);
	let response = callback(
		&fixture,
		"browser-a",
		&started.browser,
		&format!("state={}&code={code}", started.state),
	)
	.await;
	assert_eq!(
		location(&response),
		"/settings?view=providerCredentials&provider_authorization=unbound"
	);
	let unbound = credentials(&fixture)
		.await
		.into_iter()
		.find(|row| row["state"] == "active" && row["id"] != rows[0]["id"])
		.expect("the losing connection is stored")["id"]
		.clone();
	assert_eq!(bound_credential(&fixture).await, rows[0]["id"]);

	// A binding to a revoked Provider Credential serves no Run, so the next
	// connection replaces it instead of leaving the new key unbound.
	let (status, revoked) = common::request(
		&fixture.app,
		&fixture.f.config.api_token,
		"POST",
		&format!(
			"/api/tenants/alpha/provider-credentials/{}/revoke",
			rows[0]["id"].as_str().unwrap()
		),
		json!({"expected_revision": rows[0]["revision"]}),
	)
	.await;
	assert_eq!(status, 200, "{revoked}");
	assert_eq!(bound_credential(&fixture).await, rows[0]["id"]);
	let started = start(&fixture, "browser-a", json!({}), Some(&started.browser)).await;
	let (code, _) = fixture.provider.consent(&started.challenge, false);
	let response = callback(
		&fixture,
		"browser-a",
		&started.browser,
		&format!("state={}&code={code}", started.state),
	)
	.await;
	assert_eq!(
		location(&response),
		"/settings?view=providerCredentials&provider_authorization=connected"
	);
	let connected = credentials(&fixture)
		.await
		.into_iter()
		.find(|row| row["state"] == "active" && row["id"] != unbound)
		.expect("the new connection is active");
	assert_ne!(connected["id"], rows[0]["id"]);
	assert_eq!(bound_credential(&fixture).await, connected["id"]);
	common::cleanup(fixture.f, &fixture.url, &fixture.schema).await;
}

/// The Provider Credential ID the Tenant's OpenRouter binding names.
async fn bound_credential(fixture: &Fixture) -> Value {
	let binding = send(
		&fixture.app,
		"GET",
		"/api/tenants/alpha/provider-credential-bindings/openrouter",
		&[(
			"authorization",
			&format!("Bearer {}", fixture.f.config.api_token),
		)],
		Value::Null,
	)
	.await;
	let binding: Value = serde_json::from_slice(binding.body()).unwrap();
	binding["provider_credential_id"].clone()
}

#[rstest::rstest]
#[tokio::test]
async fn replayed_cross_session_and_expired_states_and_rejected_codes_store_nothing(
	#[from(parts)] parts: Parts,
	#[future(awt)]
	#[from(authorization_application)]
	#[with(parts.clone())]
	application: common::ApplicationFixture,
) {
	let fixture = Fixture::new(application, parts);

	// Cross-session: the state belongs to session A, the callback carries B.
	let started = start(&fixture, "browser-a", json!({}), None).await;
	let (code, _) = fixture.provider.consent(&started.challenge, false);
	let response = callback(
		&fixture,
		"browser-b",
		&started.browser,
		&format!("state={}&code={code}", started.state),
	)
	.await;
	assert_eq!(response.status(), 401);
	// The rejected callback consumed the state; replaying it from A fails too.
	let response = callback(
		&fixture,
		"browser-a",
		&started.browser,
		&format!("state={}&code={code}", started.state),
	)
	.await;
	assert_eq!(response.status(), 401);

	// Cross-browser: same session, but another browser's binding cookie.
	let started = start(&fixture, "browser-a", json!({}), None).await;
	let (code, _) = fixture.provider.consent(&started.challenge, false);
	let response = callback(
		&fixture,
		"browser-a",
		"another-browser",
		&format!("state={}&code={code}", started.state),
	)
	.await;
	assert_eq!(response.status(), 401);

	// Replay after success: the second use of a state fails.
	let started = start(&fixture, "browser-a", json!({}), None).await;
	let (code, _) = fixture.provider.consent(&started.challenge, false);
	let query = format!("state={}&code={code}", started.state);
	let response = callback(&fixture, "browser-a", &started.browser, &query).await;
	assert_eq!(response.status(), 303);
	assert_eq!(credentials(&fixture).await.len(), 1);
	let (fresh, _) = fixture.provider.consent(&started.challenge, false);
	let response = callback(
		&fixture,
		"browser-a",
		&started.browser,
		&format!("state={}&code={fresh}", started.state),
	)
	.await;
	assert_eq!(response.status(), 401);
	assert_eq!(credentials(&fixture).await.len(), 1);
	assert_eq!(stored_versions(&fixture), 1);

	// Wrong verifier: a code issued for another challenge is refused.
	let started = start(&fixture, "browser-a", json!({}), None).await;
	let (code, _) = fixture.provider.consent("another-challenge", false);
	let response = callback(
		&fixture,
		"browser-a",
		&started.browser,
		&format!("state={}&code={code}", started.state),
	)
	.await;
	assert_eq!(
		location(&response),
		"/settings?view=providerCredentials&provider_authorization=failed"
	);

	// Expired code at the provider.
	let started = start(&fixture, "browser-a", json!({}), None).await;
	let (code, _) = fixture.provider.consent(&started.challenge, true);
	let response = callback(
		&fixture,
		"browser-a",
		&started.browser,
		&format!("state={}&code={code}", started.state),
	)
	.await;
	assert_eq!(
		location(&response),
		"/settings?view=providerCredentials&provider_authorization=failed"
	);

	// Expired pending authorization at the host.
	let started = start(&fixture, "browser-a", json!({}), None).await;
	let (code, _) = fixture.provider.consent(&started.challenge, false);
	let expire = Query::update()
		.table(Alias::new("provider_authorizations"))
		.value_expr(
			Alias::new("expires_at"),
			Expr::cust("clock_timestamp()-interval '1 second'"),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&expire)
		.execute(fixture.f.store.pool.driver())
		.await
		.unwrap();
	let response = callback(
		&fixture,
		"browser-a",
		&started.browser,
		&format!("state={}&code={code}", started.state),
	)
	.await;
	assert_eq!(response.status(), 401);

	// A code without `state` never reaches the exchange.
	let started = start(&fixture, "browser-a", json!({}), None).await;
	let (code, _) = fixture.provider.consent(&started.challenge, false);
	let response = callback(
		&fixture,
		"browser-a",
		&started.browser,
		&format!("code={code}"),
	)
	.await;
	assert_eq!(response.status(), 401);
	assert_eq!(pending_authorizations(&fixture).await, 1);

	// A declined consent returns without a code and, as at OpenRouter, without
	// `state`; the browser binding and session still correlate it.
	let response = callback(&fixture, "browser-b", &started.browser, "").await;
	assert_eq!(response.status(), 401);
	let response = callback(&fixture, "browser-a", "another-browser", "").await;
	assert_eq!(response.status(), 401);
	let response = callback(&fixture, "browser-a", &started.browser, "").await;
	assert_eq!(
		location(&response),
		"/settings?view=providerCredentials&provider_authorization=denied"
	);
	assert_eq!(pending_authorizations(&fixture).await, 0);
	let response = callback(&fixture, "browser-a", &started.browser, "").await;
	assert_eq!(response.status(), 401);

	// A second start in the same browser and session supersedes the first, so
	// a denial never has to guess which flow it answers. The superseded `state`
	// is refused before any exchange.
	let older = start(&fixture, "browser-a", json!({}), Some(&started.browser)).await;
	let newer = start(
		&fixture,
		"browser-a",
		json!({"return_to": "/settings"}),
		Some(&started.browser),
	)
	.await;
	assert_eq!(pending_authorizations(&fixture).await, 1);
	let (code, _) = fixture.provider.consent(&older.challenge, false);
	let response = callback(
		&fixture,
		"browser-a",
		&older.browser,
		&format!("state={}&code={code}", older.state),
	)
	.await;
	assert_eq!(response.status(), 401);
	let response = callback(&fixture, "browser-a", &newer.browser, "").await;
	assert_eq!(
		location(&response),
		"/settings?provider_authorization=denied"
	);
	assert_eq!(pending_authorizations(&fixture).await, 0);

	let rows = credentials(&fixture).await;
	assert_eq!(
		rows.iter().filter(|row| row["state"] == "active").count(),
		1
	);
	assert_eq!(stored_versions(&fixture), 1);
	common::cleanup(fixture.f, &fixture.url, &fixture.schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn only_browser_sessions_start_and_return_paths_stay_local(
	#[from(parts)] parts: Parts,
	#[future(awt)]
	#[from(authorization_application)]
	#[with(parts.clone())]
	application: common::ApplicationFixture,
) {
	let fixture = Fixture::new(application, parts);
	// The Deployment Administrator bearer has no browser to bind.
	let response = send(
		&fixture.app,
		"POST",
		"/api/tenants/alpha/provider-authorizations/openrouter",
		&[
			("content-type", "application/json"),
			(
				"authorization",
				&format!("Bearer {}", fixture.f.config.api_token),
			),
		],
		json!({}),
	)
	.await;
	assert_eq!(response.status(), 403);
	// Key Material is not an accepted field.
	let response = send(
		&fixture.app,
		"POST",
		"/api/tenants/alpha/provider-authorizations/openrouter",
		&[
			("content-type", "application/json"),
			("cookie", "aidash-session=browser-a"),
			("origin", ORIGIN),
			("x-aidash-csrf", "csrf-a"),
			("x-aidash-context", "deployment_administrator"),
		],
		json!({"key_material": "sk-or-v1-pasted"}),
	)
	.await;
	assert!(response.status().is_client_error());
	for return_to in ["https://attacker.example/", "//attacker.example/"] {
		let response = send(
			&fixture.app,
			"POST",
			"/api/tenants/alpha/provider-authorizations/openrouter",
			&[
				("content-type", "application/json"),
				("cookie", "aidash-session=browser-a"),
				("origin", ORIGIN),
				("x-aidash-csrf", "csrf-a"),
				("x-aidash-context", "deployment_administrator"),
			],
			json!({"return_to": return_to}),
		)
		.await;
		assert!(response.status().is_client_error(), "{return_to}");
	}
	assert_eq!(credentials(&fixture).await.len(), 0);
	common::cleanup(fixture.f, &fixture.url, &fixture.schema).await;
}

async fn pending_authorizations(fixture: &Fixture) -> i64 {
	let query = Query::select()
		.expr(Expr::cust("count(*)"))
		.from(Alias::new("provider_authorizations"))
		.to_string(PostgresQueryBuilder);
	sqlx::query_scalar(&query)
		.fetch_one(fixture.f.store.pool.driver())
		.await
		.unwrap()
}

#[rstest::rstest]
#[tokio::test]
async fn mapped_subjects_need_the_provider_credential_management_permission(
	#[from(parts)] parts: Parts,
	#[future(awt)]
	#[from(authorization_application)]
	#[with(parts.clone())]
	application: common::ApplicationFixture,
) {
	let fixture = Fixture::new(application, parts);
	// Map the browser identity to Tenant subject alice through the normal
	// registration and Deployment Administrator approval.
	let registration = send(
		&fixture.app,
		"POST",
		"/auth/registration",
		&[
			("cookie", "aidash-session=browser-a"),
			("origin", ORIGIN),
			("x-aidash-csrf", "csrf-a"),
		],
		Value::Null,
	)
	.await;
	let registration: Value = serde_json::from_slice(registration.body()).unwrap();
	let mapping = send(
		&fixture.app,
		"POST",
		&format!(
			"/api/dashboard/registrations/{}/approve",
			registration["id"].as_str().unwrap()
		),
		&[
			("content-type", "application/json"),
			(
				"authorization",
				&format!("Bearer {}", fixture.f.config.api_token),
			),
		],
		json!({"tenant":"alpha","subject":"alice"}),
	)
	.await;
	let status = mapping.status();
	let mapping: Value = serde_json::from_slice(mapping.body()).unwrap();
	assert_eq!(status, 200, "{mapping}");
	let context = format!("mapping:{}", mapping["id"].as_str().unwrap());

	// Without provider_credential.create the start is refused and reserves nothing.
	let denied = start_request(&fixture, "browser-a", &context, json!({}), None).await;
	assert_eq!(denied.status(), 403);
	assert_eq!(pending_authorizations(&fixture).await, 0);

	// With the management permission the same subject connects OpenRouter.
	let policy: PolicyBundle = serde_json::from_value(json!({
		"tenant":"alpha","subjects":{"alice":{"kind":"user"}},
		"policies":[{"id":"provider-connections","effect":"allow","subjects":{"ids":["alice"]},
			"actions":["provider_credential.create","provider_credential.read","provider_credential_binding.read","provider_credential_binding.update"],
			"resources":{"kinds":["provider_credential","provider_credential_binding"]}}]
	}))
	.unwrap();
	Authorization {
		pool: fixture.f.store.pool.clone(),
	}
	.replace("alpha", 1, policy, "deployment_administrator")
	.await
	.unwrap();
	let started = start_as(&fixture, "browser-a", &context, json!({}), None).await;
	let (code, _) = fixture.provider.consent(&started.challenge, false);
	let response = callback(
		&fixture,
		"browser-a",
		&started.browser,
		&format!("state={}&code={code}", started.state),
	)
	.await;
	assert_eq!(
		location(&response),
		"/settings?view=providerCredentials&provider_authorization=connected"
	);
	let rows = credentials(&fixture).await;
	assert_eq!(rows.len(), 1);
	assert_eq!(rows[0]["state"], "active");
	common::cleanup(fixture.f, &fixture.url, &fixture.schema).await;
}
