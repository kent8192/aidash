//! Provider Authorization through the production router against a fake
//! OpenRouter that enforces PKCE, single-use codes and code expiry.
#[path = "../../execution/tests/support/legacy.rs"]
mod common;
use aidash_application::{
	Result,
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
use axum::{
	Json, Router,
	body::{Body, to_bytes},
	http::{Request, StatusCode},
	response::Response,
	routing::post,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use common::{TestEnvironment, test_environment};
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex},
};
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://127.0.0.1:8080";
const CALLBACK: &str = "/auth/provider-authorizations/openrouter/callback";

/// Issued codes by value: the S256 challenge they are bound to, whether they
/// have expired, and the Key Material they exchange for.
#[derive(Clone, Default)]
struct FakeOpenRouter {
	codes: Arc<Mutex<BTreeMap<String, (String, bool, String)>>>,
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
	fn router(&self) -> Router {
		let codes = self.codes.clone();
		Router::new().route(
			"/auth/keys",
			post(move |Json(body): Json<Value>| {
				let codes = codes.clone();
				async move {
					let code = body["code"].as_str().unwrap_or_default();
					let verifier = body["code_verifier"].as_str().unwrap_or_default();
					assert_eq!(body["code_challenge_method"], "S256");
					// Codes are single-use whether or not the exchange succeeds.
					let Some((challenge, expired, key)) = codes.lock().unwrap().remove(code) else {
						return (StatusCode::FORBIDDEN, Json(json!({"error":"invalid code"})));
					};
					if expired {
						return (StatusCode::FORBIDDEN, Json(json!({"error":"expired"})));
					}
					if URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) != challenge {
						return (
							StatusCode::FORBIDDEN,
							Json(json!({"error":"invalid verifier"})),
						);
					}
					(
						StatusCode::OK,
						Json(json!({"key": key, "user_id": "fixture"})),
					)
				}
			}),
		)
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
struct Validator;
#[async_trait]
impl KeyValidator for Validator {
	async fn validate(&self, _: Provider, _: &SecretString) -> Result<Validation> {
		Ok(Validation { warnings: vec![] })
	}
}

struct ProviderServer(tokio::task::JoinHandle<()>);
impl Drop for ProviderServer {
	fn drop(&mut self) {
		self.0.abort();
	}
}

struct Fixture {
	f: Federation,
	app: Router,
	provider: FakeOpenRouter,
	store: Arc<MemoryStore>,
	_server: ProviderServer,
	url: String,
	schema: String,
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

async fn setup(environment: &TestEnvironment) -> Fixture {
	let (mut f, url, schema) = common::setup(environment).await;
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
			.into_table(Alias::new("dashboard_operator_grants"))
			.columns([Alias::new("identity_id")])
			.from_subquery(Query::select().expr(Expr::val(identity)).to_owned())
			.to_string(PostgresQueryBuilder),
	] {
		sqlx::query(&query)
			.execute(f.store.pool.driver())
			.await
			.unwrap();
	}
	insert_session(&f, identity, "browser-a", "csrf-a").await;
	insert_session(&f, identity, "browser-b", "csrf-b").await;
	let policy: PolicyBundle = serde_json::from_value(json!({
		"tenant":"alpha","subjects":{"alice":{"kind":"user"}},"policies":[]
	}))
	.unwrap();
	Authorization {
		pool: f.store.pool.clone(),
	}
	.replace("alpha", 0, policy, "operator")
	.await
	.unwrap();
	let provider = FakeOpenRouter::default();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let base = format!("http://{}", listener.local_addr().unwrap());
	let router = provider.router();
	let server = ProviderServer(tokio::spawn(async move {
		axum::serve(listener, router).await.unwrap();
	}));
	let store = Arc::new(MemoryStore::default());
	f.store.provider_credentials = Some(Arc::new(Service {
		repository: Arc::new(NativeRepository {
			pool: f.store.control_pool.clone(),
			node: f.store.node_id.clone(),
		}),
		store: store.clone(),
		validator: Arc::new(Validator),
		exchange: Arc::new(
			aidash_integrations::provider_authorization::OpenRouterAuthorization::at(
				reqwest::Client::new(),
				base,
			),
		),
		fingerprint_key: "test-fingerprint-key-that-is-at-least-32-bytes".into(),
		max_per_tenant: 5,
	}));
	let app = common::application_with_settings(
		f.clone(),
		aidash_server::http::Settings {
			auth_burst: 10000,
			..Default::default()
		},
	)
	.await
	.test_transport();
	Fixture {
		f,
		app,
		provider,
		store,
		_server: server,
		url,
		schema,
	}
}

async fn send(
	app: &Router,
	method: &str,
	path: &str,
	headers: &[(&str, &str)],
	body: Value,
) -> Response {
	let mut request = Request::builder().method(method).uri(path);
	for (name, value) in headers {
		request = request.header(*name, *value);
	}
	app.clone()
		.oneshot(request.body(Body::from(body.to_string())).unwrap())
		.await
		.unwrap()
}
fn cookie(response: &Response, name: &str) -> Option<String> {
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
	let response = send(
		&fixture.app,
		"POST",
		"/api/tenants/alpha/provider-authorizations/openrouter",
		&[
			("content-type", "application/json"),
			("cookie", &cookies),
			("origin", ORIGIN),
			("x-aidash-csrf", csrf),
			("x-aidash-context", "operator"),
		],
		body,
	)
	.await;
	assert_eq!(response.headers()["cache-control"], "no-store");
	let browser =
		cookie(&response, "aidash-provider-authorization").expect("start binds the browser");
	let status = response.status();
	let bytes = to_bytes(response.into_body(), 65_536).await.unwrap();
	assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
	let started: Value = serde_json::from_slice(&bytes).unwrap();
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
async fn callback(fixture: &Fixture, session: &str, browser: &str, query: &str) -> Response {
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
fn location(response: &Response) -> &str {
	response.headers()["location"].to_str().unwrap()
}
async fn credentials(fixture: &Fixture) -> Vec<Value> {
	let response = send(
		&fixture.app,
		"GET",
		"/api/tenants/alpha/provider-credentials",
		&[("authorization", "Bearer operator-execution-fixture")],
		Value::Null,
	)
	.await;
	let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
	serde_json::from_slice(&bytes).unwrap()
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
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let fixture = setup(&environment).await;
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
	let binding = send(
		&fixture.app,
		"GET",
		"/api/tenants/alpha/provider-credential-bindings/openrouter",
		&[("authorization", "Bearer operator-execution-fixture")],
		Value::Null,
	)
	.await;
	let binding: Value =
		serde_json::from_slice(&to_bytes(binding.into_body(), 65_536).await.unwrap()).unwrap();
	assert_eq!(binding["provider_credential_id"], row["id"]);

	// Reconnect rotates the same record to a newly issued key.
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
		"/settings?provider_authorization=reconnected"
	);
	let rows = credentials(&fixture).await;
	assert_eq!(rows.len(), 1);
	assert!(rows[0]["rotated_at"].is_string());
	assert_eq!(stored_versions(&fixture), 2);
	// Key Material never reaches PostgreSQL in any table.
	let mut dump = String::new();
	for table in ["provider_authorizations", "provider_credentials", "events"] {
		let text: String = sqlx::query_scalar(&format!(
			"SELECT coalesce(string_agg(t::text, ''), '') FROM {table} t"
		))
		.fetch_one(fixture.f.store.pool.driver())
		.await
		.unwrap();
		dump.push_str(&text);
	}
	let rotated_digest: String = Sha256::digest(rotated_key.as_bytes())
		.iter()
		.map(|b| format!("{b:02x}"))
		.collect();
	assert!(dump.contains(&rotated_digest) && !dump.contains(&digest));
	assert!(!dump.contains(&key) && !dump.contains(&rotated_key));
	common::cleanup(fixture.f, &fixture.url, &fixture.schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn replayed_cross_session_and_expired_states_and_rejected_codes_store_nothing(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let fixture = setup(&environment).await;

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
	sqlx::query(
		"UPDATE provider_authorizations SET expires_at = clock_timestamp() - interval '1 second'",
	)
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

	// A declined consent returns without a code.
	let started = start(&fixture, "browser-a", json!({}), None).await;
	let response = callback(
		&fixture,
		"browser-a",
		&started.browser,
		&format!("state={}", started.state),
	)
	.await;
	assert_eq!(
		location(&response),
		"/settings?view=providerCredentials&provider_authorization=denied"
	);

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
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let fixture = setup(&environment).await;
	// The operator bearer has no browser to bind.
	let response = send(
		&fixture.app,
		"POST",
		"/api/tenants/alpha/provider-authorizations/openrouter",
		&[
			("content-type", "application/json"),
			("authorization", "Bearer operator-execution-fixture"),
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
			("x-aidash-context", "operator"),
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
				("x-aidash-context", "operator"),
			],
			json!({"return_to": return_to}),
		)
		.await;
		assert!(response.status().is_client_error(), "{return_to}");
	}
	assert_eq!(credentials(&fixture).await.len(), 0);
	common::cleanup(fixture.f, &fixture.url, &fixture.schema).await;
}
