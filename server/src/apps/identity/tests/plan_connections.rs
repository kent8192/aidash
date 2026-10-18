//! Plan Connections through the production router and PostgreSQL (#159,
//! ADR 0007, ADR 0031). An in-process fake OpenAI enforces PKCE, the nonce and
//! single-use codes; the HTTP adapter itself is exercised against a fake
//! provider in `aidash-integrations`.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../execution/tests/support/legacy.rs"]
mod common;
use aidash_application::{
	Error, Result,
	plan_connections::{Authorization, ExchangeRequest, Issued, RefreshFailure, Service},
	provider_access::{Context, KeyMaterialReader, ProviderAccess, Source},
	provider_credentials::Store,
	tenancy::TenancyScope,
};
use aidash_domain::plan_connections::{Failure, PlanModel, SCOPES};
use aidash_domain::provider_credentials::Provider;
use aidash_server::{
	apps::identity::repositories::{
		plan_connections::{NativeRepository, admit, pinned},
		provider_credentials::AdmittedAccess,
	},
	authorization::{Authorization as Policies, policy::PolicyBundle},
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
	sync::{Arc, Mutex},
};
use uuid::Uuid;

const ORIGIN: &str = "http://127.0.0.1:8080";
const PLAN: &str = "/api/tenants/alpha/plan-connections/chatgpt";

/// One consent: the challenge and nonce its code is bound to.
struct Consent {
	challenge: String,
	nonce: String,
	client: String,
}
#[derive(Default)]
struct Calls {
	exchanges: usize,
	refreshes: usize,
	revoked: Vec<String>,
}
#[derive(Clone, Default)]
struct FakeOpenAi {
	codes: Arc<Mutex<BTreeMap<String, Consent>>>,
	/// The nonce each issued ID token carries.
	id_tokens: Arc<Mutex<BTreeMap<String, String>>>,
	calls: Arc<Mutex<Calls>>,
}
impl FakeOpenAi {
	/// What OpenAI does after consent: bind a fresh code to the request.
	fn consent(&self, authorization_url: &str) -> (String, String, String) {
		let url = reqwest::Url::parse(authorization_url).unwrap();
		let query: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
		let client = if query["client_id"] == "dynamic_agent_client" {
			format!("oaiapp_{}", Uuid::new_v4().simple())
		} else {
			query["client_id"].clone()
		};
		let code = format!("code-{}", Uuid::new_v4().simple());
		self.codes.lock().unwrap().insert(
			code.clone(),
			Consent {
				challenge: query["code_challenge"].clone(),
				nonce: query["nonce"].clone(),
				client: client.clone(),
			},
		);
		(code, query["state"].clone(), client)
	}
}
fn tokens(n: usize, id_token: Option<String>) -> Issued {
	Issued {
		access_token: format!("plan-access-{n}").into(),
		refresh_token: format!("plan-refresh-{n}").into(),
		id_token: id_token.map(SecretString::from),
		expires_in: 3600,
		scope: Some(SCOPES.into()),
		earliest_refresh_at: None,
	}
}
#[async_trait]
impl Authorization for FakeOpenAi {
	async fn exchange(&self, request: ExchangeRequest<'_>) -> Result<Issued> {
		let rejected = || Error::Invalid("Sign in with ChatGPT rejected the authorization".into());
		// Codes are single-use whether or not the exchange succeeds.
		let consent = self
			.codes
			.lock()
			.unwrap()
			.remove(request.code)
			.ok_or_else(rejected)?;
		if URL_SAFE_NO_PAD.encode(Sha256::digest(request.verifier.expose_secret().as_bytes()))
			!= consent.challenge
			|| request.client_id != consent.client
		{
			return Err(rejected());
		}
		let mut calls = self.calls.lock().unwrap();
		calls.exchanges += 1;
		let id_token = format!("id-token-{}", calls.exchanges);
		self.id_tokens
			.lock()
			.unwrap()
			.insert(id_token.clone(), consent.nonce);
		Ok(tokens(calls.exchanges, Some(id_token)))
	}
	async fn verify_id_token(
		&self,
		id_token: &SecretString,
		_: &str,
		nonce: &str,
	) -> Result<String> {
		match self.id_tokens.lock().unwrap().get(id_token.expose_secret()) {
			Some(expected) if expected == nonce => Ok("chatgpt-account-1".into()),
			_ => Err(Error::Invalid("invalid ID token".into())),
		}
	}
	async fn refresh(
		&self,
		_: &str,
		refresh_token: &SecretString,
	) -> std::result::Result<Issued, RefreshFailure> {
		let mut calls = self.calls.lock().unwrap();
		if calls
			.revoked
			.contains(&refresh_token.expose_secret().to_owned())
		{
			return Err(RefreshFailure::Terminal);
		}
		calls.refreshes += 1;
		Ok(tokens(100 + calls.refreshes, None))
	}
	async fn revoke(&self, _: &str, refresh_token: &SecretString) -> Result<()> {
		self.calls
			.lock()
			.unwrap()
			.revoked
			.push(refresh_token.expose_secret().to_owned());
		Ok(())
	}
	async fn models(&self, access_token: &SecretString) -> Result<Vec<PlanModel>> {
		assert!(access_token.expose_secret().starts_with("plan-access-"));
		Ok(vec![PlanModel {
			slug: "gpt-5.5".into(),
			display_name: "GPT-5.5".into(),
			context_window: 400_000,
			max_output_tokens: 128_000,
			defaulted: false,
		}])
	}
}

/// A Store that keeps Plan Tokens in memory, so the test can prove they
/// never reach PostgreSQL.
#[derive(Default)]
struct MemoryStore {
	versions: Mutex<BTreeMap<String, Vec<Option<String>>>>,
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
	async fn create_plan(&self, _: &str, id: Uuid) -> Result<()> {
		self.versions
			.lock()
			.unwrap()
			.insert(self.plan_resource(id), vec![]);
		Ok(())
	}
	async fn add_version(&self, _: &str, resource: &str, key: &SecretString) -> Result<String> {
		let mut versions = self.versions.lock().unwrap();
		let entry = versions
			.get_mut(resource)
			.ok_or_else(|| Error::NotFound("resource".into()))?;
		entry.push(Some(key.expose_secret().to_owned()));
		Ok(format!("{resource}/versions/{}", entry.len()))
	}
	async fn versions(&self, _: &str, resource: &str) -> Result<Vec<String>> {
		let versions = self.versions.lock().unwrap();
		let entry = versions
			.get(resource)
			.ok_or_else(|| Error::NotFound("resource".into()))?;
		Ok(entry
			.iter()
			.enumerate()
			.filter(|(_, value)| value.is_some())
			.map(|(n, _)| format!("{resource}/versions/{}", n + 1))
			.collect())
	}
	async fn disable(&self, _: &str, _: &str) -> Result<()> {
		Ok(())
	}
	async fn destroy(&self, _: &str, version: &str) -> Result<()> {
		let (resource, n) = version.rsplit_once("/versions/").unwrap();
		let n: usize = n.parse().unwrap();
		if let Some(entry) = self.versions.lock().unwrap().get_mut(resource) {
			entry[n - 1] = None;
		}
		Ok(())
	}
	async fn delete(&self, _: &str, resource: &str) -> Result<()> {
		self.versions.lock().unwrap().remove(resource);
		Ok(())
	}
}
#[async_trait]
impl KeyMaterialReader for MemoryStore {
	async fn read(&self, _: &str, resource: &str, version: &str) -> Result<SecretString> {
		let (pinned, n) = version.rsplit_once("/versions/").unwrap();
		assert_eq!(pinned, resource);
		let n: usize = n.parse().unwrap();
		self.versions
			.lock()
			.unwrap()
			.get(resource)
			.and_then(|entry| entry.get(n - 1).cloned().flatten())
			.map(SecretString::from)
			.ok_or_else(|| Error::NotFound("version".into()))
	}
}
impl MemoryStore {
	/// Every stored plaintext, to prove which Plan Tokens are kept.
	fn plaintexts(&self) -> Vec<String> {
		self.versions
			.lock()
			.unwrap()
			.values()
			.flatten()
			.flatten()
			.cloned()
			.collect()
	}
}

/// The fake OpenAI and Store the test observes; shared with the runtime.
#[derive(Clone)]
struct Parts {
	openai: FakeOpenAi,
	store: Arc<MemoryStore>,
	enabled: bool,
}
impl Default for Parts {
	fn default() -> Self {
		Self {
			openai: FakeOpenAi::default(),
			store: Arc::default(),
			enabled: true,
		}
	}
}
#[rstest::fixture]
fn parts() -> Parts {
	Parts::default()
}
#[rstest::fixture]
fn disabled_parts() -> Parts {
	Parts {
		enabled: false,
		..Parts::default()
	}
}

async fn execute(f: &Federation, query: String) {
	sqlx::query(&query)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
}
async fn insert_identity(f: &Federation, subject: &str, deployment_administrator: bool) -> Uuid {
	let identity = Uuid::new_v4();
	execute(
		f,
		Query::insert()
			.into_table(Alias::new("dashboard_identities"))
			.columns(["id", "issuer", "subject", "last_valid_at"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::val(identity))
					.expr(Expr::val("https://accounts.google.com"))
					.expr(Expr::val(subject))
					.expr(Expr::cust("clock_timestamp()"))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.await;
	if deployment_administrator {
		execute(
			f,
			Query::insert()
				.into_table(Alias::new("dashboard_deployment_administrator_grants"))
				.columns([Alias::new("identity_id")])
				.from_subquery(Query::select().expr(Expr::val(identity)).to_owned())
				.to_string(PostgresQueryBuilder),
		)
		.await;
	}
	identity
}
async fn insert_session(f: &Federation, identity: Uuid, token: &str, csrf: &str) {
	execute(
		f,
		Query::insert()
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
			.to_string(PostgresQueryBuilder),
	)
	.await;
}

/// Alice's identity with two dashboard sessions, Bob's with one, and a
/// Provider Credential Store whose Sign in with ChatGPT is the fake OpenAI.
#[rstest::fixture]
fn plan_runtime(
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
		let alice = insert_identity(f, "plan-connection-alice", true).await;
		insert_session(f, alice, "browser-a", "csrf-a").await;
		insert_session(f, alice, "browser-b", "csrf-b").await;
		let bob = insert_identity(f, "plan-connection-bob", false).await;
		insert_session(f, bob, "browser-c", "csrf-c").await;
		// A Tenant's first policy needs its root record (#304).
		common::tenant_root(f, "alpha").await;
		let policy: PolicyBundle = serde_json::from_value(json!({
			"tenant":"alpha","subjects":{"alice":{"kind":"user"},"carol":{"kind":"user"}},"policies":[]
		}))
		.unwrap();
		Policies {
			pool: f.store.pool.clone(),
		}
		.replace("alpha", 0, policy, "deployment_administrator")
		.await
		.unwrap();
		f.store.plan_connections = Some(Arc::new(Service {
			repository: Arc::new(NativeRepository {
				pool: f.store.control_pool.clone(),
				node: f.store.node_id.clone(),
			}),
			store: parts.store.clone(),
			reader: parts.store.clone(),
			authorization: Arc::new(parts.openai.clone()),
		}));
		f.store.plan_connections_enabled = parts.enabled;
		f.registry = f.registry.clone().with_plan_connections(parts.enabled);
		owner
	}
	.boxed()
	.shared()
}
#[rstest::fixture]
fn plan_application(
	#[default(Parts::default())] _parts: Parts,
	#[from(plan_runtime)]
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
	openai: FakeOpenAi,
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
			openai: parts.openai,
			store: parts.store,
			url,
			schema,
		}
	}
	async fn cleanup(self) {
		common::cleanup(self.f, &self.url, &self.schema).await;
	}
}

fn csrf(session: &str) -> &'static str {
	match session {
		"browser-a" => "csrf-a",
		"browser-b" => "csrf-b",
		_ => "csrf-c",
	}
}
async fn send(
	fixture: &Fixture,
	session: &str,
	context: &str,
	method: &str,
	path: &str,
	body: Value,
) -> TestResponse {
	let cookie = format!("aidash-session={session}");
	let body = if body.is_null() {
		Vec::new()
	} else {
		body.to_string().into_bytes()
	};
	common::http_response(
		&fixture.app,
		method,
		path,
		&[
			("content-type", "application/json"),
			("cookie", &cookie),
			("origin", ORIGIN),
			("x-aidash-csrf", csrf(session)),
			("x-aidash-context", context),
		],
		&body,
	)
	.await
}
fn json_body(response: &TestResponse) -> Value {
	serde_json::from_slice(response.body()).unwrap_or(Value::Null)
}
/// Maps the session's identity to `subject` through the normal registration
/// and Deployment Administrator approval, and returns its dashboard context.
async fn map(fixture: &Fixture, session: &str, subject: &str) -> (u16, String) {
	let registration = common::http_response(
		&fixture.app,
		"POST",
		"/auth/registration",
		&[
			("cookie", &format!("aidash-session={session}")),
			("origin", ORIGIN),
			("x-aidash-csrf", csrf(session)),
		],
		&[],
	)
	.await;
	let registration = json_body(&registration);
	let (status, mapping) = common::request(
		&fixture.app,
		&fixture.f.config.api_token,
		"POST",
		&format!(
			"/api/dashboard/registrations/{}/approve",
			registration["id"].as_str().unwrap()
		),
		json!({"tenant":"alpha","subject":subject}),
	)
	.await;
	let context = mapping["id"]
		.as_str()
		.map(|id| format!("mapping:alpha:{id}"))
		.unwrap_or_default();
	(status, context)
}
/// Starts a pasted-URL authorization and consents at the fake OpenAI; returns
/// the address the browser fails to load.
async fn consent(fixture: &Fixture, session: &str, context: &str) -> String {
	let response = send(
		fixture,
		session,
		context,
		"POST",
		&format!("{PLAN}/authorizations"),
		json!({}),
	)
	.await;
	let started = json_body(&response);
	assert_eq!(response.status(), 200, "{started}");
	assert_eq!(response.headers()["cache-control"], "no-store");
	assert_eq!(
		started["redirect_uri"],
		"http://127.0.0.1:1457/auth/callback"
	);
	let url = started["authorization_url"].as_str().unwrap();
	let query: BTreeMap<_, _> = reqwest::Url::parse(url)
		.unwrap()
		.query_pairs()
		.into_owned()
		.collect();
	assert!(query["ext_agent_host_id"].starts_with("urn:uuid:"));
	assert_eq!(query["redirect_uri"], started["redirect_uri"]);
	let (code, state, client) = fixture.openai.consent(url);
	format!("http://127.0.0.1:1457/auth/callback?code={code}&state={state}&client_id={client}")
}
async fn complete(fixture: &Fixture, session: &str, context: &str, callback: &str) -> TestResponse {
	send(
		fixture,
		session,
		context,
		"POST",
		&format!("{PLAN}/authorizations/complete"),
		json!({"callback_url": callback}),
	)
	.await
}
async fn connect(fixture: &Fixture, session: &str, context: &str) -> Value {
	let callback = consent(fixture, session, context).await;
	let response = complete(fixture, session, context, &callback).await;
	let completed = json_body(&response);
	assert_eq!(response.status(), 200, "{completed}");
	completed
}
async fn table_text(f: &Federation, table: &str) -> String {
	let query = Query::select()
		.expr_as(
			Expr::cust("coalesce(string_agg(t::text, ''), '')"),
			Alias::new("dump"),
		)
		.from_as(Alias::new(table), Alias::new("t"))
		.to_string(PostgresQueryBuilder);
	sqlx::query_scalar(&query)
		.fetch_one(f.store.pool.fixture_driver())
		.await
		.unwrap()
}

/// A disconnect ends every pending attempt, whether it would reconnect or
/// connect for the first time, so a callback that arrives afterwards
/// connects nothing.
#[rstest::rstest]
#[tokio::test]
async fn a_disconnect_ends_the_pending_attempts_of_its_owner(
	#[from(parts)] parts: Parts,
	#[future(awt)]
	#[from(plan_application)]
	#[with(parts.clone())]
	application: common::ApplicationFixture,
) {
	let fixture = Fixture::new(application, parts);
	let (status, context) = map(&fixture, "browser-a", "alice").await;
	assert_eq!(status, 200);
	// A first-time attempt is pending when the owner disconnects; there is
	// nothing to disconnect yet, but the attempt still ends.
	let first = consent(&fixture, "browser-a", &context).await;
	let none = send(
		&fixture,
		"browser-a",
		&context,
		"DELETE",
		PLAN,
		json!({"expected_revision": 1}),
	)
	.await;
	assert_eq!(none.status(), 404);
	assert_eq!(
		complete(&fixture, "browser-a", &context, &first)
			.await
			.status(),
		401
	);
	// A reconnect attempt is pending when the owner disconnects.
	let connected = connect(&fixture, "browser-a", &context).await;
	let late = consent(&fixture, "browser-a", &context).await;
	let disconnected = send(
		&fixture,
		"browser-a",
		&context,
		"DELETE",
		PLAN,
		json!({"expected_revision": connected["connection"]["revision"]}),
	)
	.await;
	assert_eq!(disconnected.status(), 200);
	assert_eq!(
		complete(&fixture, "browser-a", &context, &late)
			.await
			.status(),
		401
	);
	let status = send(&fixture, "browser-a", &context, "GET", PLAN, Value::Null).await;
	assert_eq!(json_body(&status)["connection"], Value::Null);
}

#[rstest::rstest]
#[tokio::test]
async fn owners_connect_by_pasting_the_callback_and_plan_tokens_stay_in_the_store(
	#[from(parts)] parts: Parts,
	#[future(awt)]
	#[from(plan_application)]
	#[with(parts.clone())]
	application: common::ApplicationFixture,
) {
	let fixture = Fixture::new(application, parts);
	let (status, context) = map(&fixture, "browser-a", "alice").await;
	assert_eq!(status, 200);
	let before = json_body(&send(&fixture, "browser-a", &context, "GET", PLAN, Value::Null).await);
	assert_eq!(before, json!({"enabled": true, "connection": null}));

	let completed = connect(&fixture, "browser-a", &context).await;
	assert_eq!(completed["outcome"], "connected");
	let connection = &completed["connection"];
	assert_eq!(connection["state"], "active");
	assert_eq!(connection["subject"], "alice");
	// Only the access and refresh tokens are stored; the ID token is discarded.
	let stored = fixture.store.plaintexts();
	assert_eq!(stored.len(), 1);
	assert!(stored[0].contains("plan-access-1") && !stored[0].contains("id-token"));
	// Neither Plan Tokens nor the issued client reach a response or PostgreSQL.
	let status = send(&fixture, "browser-a", &context, "GET", PLAN, Value::Null).await;
	assert_eq!(status.headers()["cache-control"], "no-store");
	let text = String::from_utf8_lossy(status.body()).to_string();
	assert!(
		!text.contains("plan-access") && !text.contains("oaiapp_"),
		"{text}"
	);
	for table in ["plan_connections", "plan_authorizations", "events"] {
		let dump = table_text(&fixture.f, table).await;
		assert!(
			!dump.contains("plan-access") && !dump.contains("plan-refresh"),
			"{table}"
		);
	}

	// The owner's model list is read with their own connection.
	let models = send(
		&fixture,
		"browser-a",
		&context,
		"GET",
		&format!("{PLAN}/models"),
		Value::Null,
	)
	.await;
	assert_eq!(models.status(), 200);
	assert_eq!(models.headers()["cache-control"], "no-store");
	assert_eq!(json_body(&models)["models"][0]["slug"], "gpt-5.5");

	// Reconnecting keeps the connection and its issued client.
	let reconnected = connect(&fixture, "browser-a", &context).await;
	assert_eq!(reconnected["outcome"], "reconnected");
	assert_eq!(reconnected["connection"]["id"], connection["id"]);

	// Disconnecting revokes the grant at OpenAI and deletes every copy.
	let revision = reconnected["connection"]["revision"].clone();
	let disconnected = send(
		&fixture,
		"browser-a",
		&context,
		"DELETE",
		PLAN,
		json!({"expected_revision": revision}),
	)
	.await;
	assert_eq!(disconnected.status(), 200);
	assert_eq!(disconnected.headers()["cache-control"], "no-store");
	assert_eq!(json_body(&disconnected), json!({"revoked": true}));
	assert!(fixture.store.plaintexts().is_empty());
	assert!(
		fixture
			.openai
			.calls
			.lock()
			.unwrap()
			.revoked
			.contains(&"plan-refresh-2".to_owned())
	);
	let after = json_body(&send(&fixture, "browser-a", &context, "GET", PLAN, Value::Null).await);
	assert_eq!(after["connection"], Value::Null);
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn callbacks_complete_only_in_the_starting_session_and_only_once(
	#[from(parts)] parts: Parts,
	#[future(awt)]
	#[from(plan_application)]
	#[with(parts.clone())]
	application: common::ApplicationFixture,
) {
	let fixture = Fixture::new(application, parts);
	let (_, context) = map(&fixture, "browser-a", "alice").await;
	let callback = consent(&fixture, "browser-a", &context).await;
	// Another session of the same person cannot complete it, and the
	// attempt is consumed.
	let other = complete(&fixture, "browser-b", &context, &callback).await;
	assert_eq!(other.status(), 401);
	let replay = complete(&fixture, "browser-a", &context, &callback).await;
	assert_eq!(replay.status(), 401);
	assert_eq!(fixture.openai.calls.lock().unwrap().exchanges, 0);

	// A URL for another redirect target is refused before any exchange.
	let callback = consent(&fixture, "browser-a", &context).await;
	let moved = callback.replace("127.0.0.1:1457", "127.0.0.1:1458");
	let refused = complete(&fixture, "browser-a", &context, &moved).await;
	assert_eq!(refused.status(), 400);
	assert_eq!(fixture.openai.calls.lock().unwrap().exchanges, 0);

	// A declined consent stores nothing.
	let callback = consent(&fixture, "browser-a", &context).await;
	let state = reqwest::Url::parse(&callback)
		.unwrap()
		.query_pairs()
		.find(|(name, _)| name == "state")
		.unwrap()
		.1
		.into_owned();
	let declined = complete(
		&fixture,
		"browser-a",
		&context,
		&format!("http://127.0.0.1:1457/auth/callback?error=access_denied&state={state}"),
	)
	.await;
	assert_eq!(declined.status(), 200);
	assert_eq!(json_body(&declined)["outcome"], "denied");
	assert!(fixture.store.plaintexts().is_empty());
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn only_the_owners_own_session_reaches_plan_connections(
	#[from(parts)] parts: Parts,
	#[future(awt)]
	#[from(plan_application)]
	#[with(parts.clone())]
	application: common::ApplicationFixture,
) {
	let fixture = Fixture::new(application, parts);
	let (_, context) = map(&fixture, "browser-a", "alice").await;
	// The Deployment Administrator context holds no Plan Connection.
	let deployment_administrator = send(
		&fixture,
		"browser-a",
		"deployment_administrator",
		"GET",
		PLAN,
		Value::Null,
	)
	.await;
	assert_eq!(deployment_administrator.status(), 403);
	// A bearer caller has no dashboard or desktop session.
	let (status, _) = common::request(
		&fixture.app,
		&fixture.f.config.api_token,
		"POST",
		&format!("{PLAN}/authorizations"),
		json!({}),
	)
	.await;
	assert_eq!(status, 403);
	// Another Tenant's path is never the owner's.
	let other = send(
		&fixture,
		"browser-a",
		&context,
		"GET",
		"/api/tenants/beta/plan-connections/chatgpt",
		Value::Null,
	)
	.await;
	assert_eq!(other.status(), 403);
	// OpenRouter is served by Provider Credentials, never a Plan Connection.
	let openrouter = send(
		&fixture,
		"browser-a",
		&context,
		"GET",
		"/api/tenants/alpha/plan-connections/openrouter",
		Value::Null,
	)
	.await;
	assert_eq!(openrouter.status(), 404);
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn turning_plan_connections_off_keeps_status_and_disconnect(
	#[from(disabled_parts)] parts: Parts,
	#[future(awt)]
	#[from(plan_application)]
	#[with(parts.clone())]
	application: common::ApplicationFixture,
) {
	let fixture = Fixture::new(application, parts);
	let (_, context) = map(&fixture, "browser-a", "alice").await;
	// A connection made while the capability was on.
	let service = fixture.f.store.plan_connections.clone().unwrap();
	let owner = aidash_application::plan_connections::Owner {
		tenant: "alpha".into(),
		subject: "alice".into(),
	};
	let request = aidash_application::plan_connections::begin(
		"urn:uuid:00000000-0000-4000-8000-000000000000",
		1457,
		None,
	)
	.unwrap();
	let (code, state, client) = fixture.openai.consent(&request.url);
	service
		.complete(
			&owner,
			identity_of(&fixture.f, "plan-connection-alice").await,
			aidash_application::plan_connections::Pending {
				state: request.state.expose_secret(),
				verifier: &request.verifier,
				nonce: request.nonce.expose_secret(),
				redirect_uri: &request.redirect_uri,
				saved_client: None,
			},
			&aidash_domain::plan_connections::Callback {
				state,
				result: aidash_domain::plan_connections::CallbackResult::Code {
					code,
					client_id: Some(client),
				},
			},
			"fixture",
		)
		.await
		.unwrap();
	let status = json_body(&send(&fixture, "browser-a", &context, "GET", PLAN, Value::Null).await);
	assert_eq!(status["enabled"], false);
	assert_eq!(status["connection"]["state"], "active");
	for (method, path) in [
		("POST", format!("{PLAN}/authorizations")),
		("GET", format!("{PLAN}/models")),
	] {
		let response = send(&fixture, "browser-a", &context, method, &path, json!({})).await;
		assert_eq!(response.status(), 404, "{method} {path}");
	}
	// Owners can still end their grant here.
	let disconnected = send(
		&fixture,
		"browser-a",
		&context,
		"DELETE",
		PLAN,
		json!({"expected_revision": status["connection"]["revision"]}),
	)
	.await;
	assert_eq!(disconnected.status(), 200);
	assert_eq!(json_body(&disconnected), json!({"revoked": true}));
	assert!(fixture.store.plaintexts().is_empty());
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn disabling_the_owners_mapping_retires_the_connection(
	#[from(parts)] parts: Parts,
	#[future(awt)]
	#[from(plan_application)]
	#[with(parts.clone())]
	application: common::ApplicationFixture,
) {
	let fixture = Fixture::new(application, parts);
	let (_, context) = map(&fixture, "browser-a", "alice").await;
	connect(&fixture, "browser-a", &context).await;
	// A second person cannot be mapped to a subject that holds a Plan
	// Connection.
	let (status, _) = map(&fixture, "browser-c", "alice").await;
	assert_eq!(status, 409);
	let mapping = context.strip_prefix("mapping:alpha:").unwrap();
	let (status, _) = common::request(
		&fixture.app,
		&fixture.f.config.api_token,
		"POST",
		&format!("/api/dashboard/mappings/{mapping}/disable"),
		json!({"expected_revision": 1}),
	)
	.await;
	assert_eq!(status, 204);
	// The grant is revoked at OpenAI and Aidash's copy is gone.
	assert!(
		fixture
			.openai
			.calls
			.lock()
			.unwrap()
			.revoked
			.contains(&"plan-refresh-1".to_owned())
	);
	assert!(fixture.store.plaintexts().is_empty());
	assert_eq!(table_text(&fixture.f, "plan_connections").await, "");
	// Now another person may be mapped to the subject.
	let (status, _) = map(&fixture, "browser-c", "alice").await;
	assert_eq!(status, 200);
	fixture.cleanup().await;
}

/// A connection no enabled Mapping reaches, such as one whose retirement
/// failed, is retired before the subject is mapped again.
#[rstest::rstest]
#[tokio::test]
async fn approvals_retire_a_connection_left_without_a_mapping(
	#[from(parts)] parts: Parts,
	#[future(awt)]
	#[from(plan_application)]
	#[with(parts.clone())]
	application: common::ApplicationFixture,
) {
	let fixture = Fixture::new(application, parts);
	let (_, context) = map(&fixture, "browser-a", "alice").await;
	connect(&fixture, "browser-a", &context).await;
	let mapping = context.strip_prefix("mapping:alpha:").unwrap();
	// Disable the Mapping behind the service's back, as a failed retirement
	// would leave it.
	execute(
		&fixture.f,
		format!("UPDATE dashboard_mappings SET enabled = false WHERE id = '{mapping}'"),
	)
	.await;
	assert!(
		table_text(&fixture.f, "plan_connections")
			.await
			.contains("alice")
	);
	let (status, _) = map(&fixture, "browser-c", "alice").await;
	assert_eq!(status, 200);
	assert_eq!(table_text(&fixture.f, "plan_connections").await, "");
	assert!(fixture.store.plaintexts().is_empty());
	fixture.cleanup().await;
}

/// The identity whose External Identity subject is `subject`.
async fn identity_of(f: &Federation, subject: &str) -> Uuid {
	let query = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("dashboard_identities"))
		.and_where(Expr::col(Alias::new("subject")).eq(subject))
		.to_string(PostgresQueryBuilder);
	sqlx::query_scalar(&query)
		.fetch_one(f.store.pool.driver())
		.await
		.unwrap()
}

/// Sets or clears the Identity Disablement of the identity whose External
/// Identity subject is `subject`.
async fn set_disabled(f: &Federation, subject: &str, disabled: bool) {
	let value = if disabled {
		"clock_timestamp()"
	} else {
		"NULL"
	};
	execute(
		f,
		format!(
			"UPDATE dashboard_identities SET disabled_at = {value} WHERE subject = '{subject}'"
		),
	)
	.await;
}

/// Admits `run` as a Run whose delegation chain starts at alice.
async fn admit_alice(
	f: &Federation,
	snapshot: &aidash_domain::registry::bindings::BindingSnapshot,
	run: Uuid,
) -> aidash_server::Result<()> {
	let grant = aidash_domain::identity::execution::ExecutionGrant {
		run_id: run,
		task_id: Uuid::new_v4(),
		workspace_id: Uuid::new_v4(),
		tenant: "alpha".into(),
		credential_id: Uuid::new_v4(),
		root_subject: "alice".into(),
		subject_chain: vec!["alice".into()],
	};
	let mut tx = f
		.store
		.control_pool
		.begin(&TenancyScope::tenant("alpha").unwrap())
		.await
		.unwrap();
	let result = admit(tx.as_mut(), &f.store, &grant, snapshot, false).await;
	if result.is_ok() {
		tx.commit().await.unwrap();
	}
	result
}

/// Identities A (alice's) and B (bob's) are both mapped to the subject alice
/// while B is disabled, so A connects. Disabling A and restoring B leaves B
/// the only active identity mapped there, yet B's Runs never use A's Plan
/// Tokens; once A's Mapping is disabled the connection is retired although
/// B's remains (#315).
#[rstest::rstest]
#[tokio::test]
async fn connections_serve_only_the_identity_that_connected_them(
	#[from(parts)] parts: Parts,
	#[future(awt)]
	#[from(plan_application)]
	#[with(parts.clone())]
	application: common::ApplicationFixture,
) {
	let fixture = Fixture::new(application, parts);
	let f = fixture.f.clone();
	let snapshot = snapshot(&f).await;
	let admit_once = |run: Uuid| admit_alice(&f, &snapshot, run);
	let (_, owners) = map(&fixture, "browser-a", "alice").await;
	let (status, _) = map(&fixture, "browser-c", "alice").await;
	assert_eq!(status, 200);
	set_disabled(&f, "plan-connection-bob", true).await;
	connect(&fixture, "browser-a", &owners).await;
	let admitted = Uuid::new_v4();
	admit_once(admitted).await.unwrap();
	let access = AdmittedAccess {
		store: f.store.clone(),
	};
	let context = Context {
		tenant: "alpha".into(),
		run: Some(admitted),
		..Context::default()
	};
	let source = Source::Plan {
		provider: "chatgpt".into(),
	};
	let endpoint = Provider::Chatgpt.base_url();
	access.check(&context, endpoint, &source).await.unwrap();

	set_disabled(&f, "plan-connection-alice", true).await;
	set_disabled(&f, "plan-connection-bob", false).await;
	assert!(matches!(
		admit_once(Uuid::new_v4()).await,
		Err(aidash_server::Error::Plan(Failure::NotConnected))
	));
	// A Run admitted before is stopped at its next call as well.
	assert!(matches!(
		access.resolve(&context, endpoint, &source).await,
		Err(Error::Plan(Failure::NotConnected))
	));
	// Identity Disablement only paused it.
	assert!(table_text(&f, "plan_connections").await.contains("alice"));

	let mapping = owners.strip_prefix("mapping:alpha:").unwrap();
	let (status, _) = common::request(
		&fixture.app,
		&f.config.api_token,
		"POST",
		&format!("/api/dashboard/mappings/{mapping}/disable"),
		json!({"expected_revision": 1}),
	)
	.await;
	assert_eq!(status, 204);
	assert_eq!(table_text(&f, "plan_connections").await, "");
	assert!(fixture.store.plaintexts().is_empty());
	assert!(
		fixture
			.openai
			.calls
			.lock()
			.unwrap()
			.revoked
			.contains(&"plan-refresh-1".to_owned())
	);
	fixture.cleanup().await;
}

/// Once B, the only active identity mapped to alice, reconnects A's Plan
/// Connection, a Run admitted for A still fails: its pin names A, which the
/// connection no longer serves, however late the call reads it (#315).
#[rstest::rstest]
#[tokio::test]
async fn runs_admitted_for_another_identity_never_use_a_reconnection(
	#[from(parts)] parts: Parts,
	#[future(awt)]
	#[from(plan_application)]
	#[with(parts.clone())]
	application: common::ApplicationFixture,
) {
	let fixture = Fixture::new(application, parts);
	let f = fixture.f.clone();
	let snapshot = snapshot(&f).await;
	let (_, owners) = map(&fixture, "browser-a", "alice").await;
	let (status, others) = map(&fixture, "browser-c", "alice").await;
	assert_eq!(status, 200);
	set_disabled(&f, "plan-connection-bob", true).await;
	connect(&fixture, "browser-a", &owners).await;
	let admitted = Uuid::new_v4();
	admit_alice(&f, &snapshot, admitted).await.unwrap();
	let pin = pinned(&f.store.control_pool, "alpha", admitted, Provider::Chatgpt)
		.await
		.unwrap();
	assert_eq!(pin.identity, identity_of(&f, "plan-connection-alice").await);

	set_disabled(&f, "plan-connection-alice", true).await;
	set_disabled(&f, "plan-connection-bob", false).await;
	let reconnected = connect(&fixture, "browser-c", &others).await;
	assert_eq!(reconnected["outcome"], "reconnected", "{reconnected}");
	let access = AdmittedAccess {
		store: f.store.clone(),
	};
	let source = Source::Plan {
		provider: "chatgpt".into(),
	};
	let endpoint = Provider::Chatgpt.base_url();
	let context = |run| Context {
		tenant: "alpha".into(),
		run: Some(run),
		..Context::default()
	};
	// The same connection, now B's: A's Run is refused at every entry.
	assert_eq!(
		pinned(&f.store.control_pool, "alpha", admitted, Provider::Chatgpt)
			.await
			.unwrap(),
		pin
	);
	assert!(matches!(
		access.check(&context(admitted), endpoint, &source).await,
		Err(Error::Plan(Failure::NotConnected))
	));
	assert!(matches!(
		access.resolve(&context(admitted), endpoint, &source).await,
		Err(Error::Plan(Failure::NotConnected))
	));
	assert!(matches!(
		access
			.plan_rejected(
				&context(admitted),
				aidash_domain::plan_connections::Disposition::RefreshThenReconnect,
			)
			.await,
		Err(Error::Plan(Failure::NotConnected))
	));
	// A Run admitted for B uses B's tokens.
	let current = Uuid::new_v4();
	admit_alice(&f, &snapshot, current).await.unwrap();
	let resolved = access
		.resolve(&context(current), endpoint, &source)
		.await
		.unwrap();
	assert_eq!(resolved.bearer.expose_secret(), "plan-access-2");
	fixture.cleanup().await;
}

/// The Binding snapshot of an Agent whose model is served by Plan Connections.
async fn snapshot(f: &Federation) -> aidash_domain::registry::bindings::BindingSnapshot {
	let model: aidash_server::registry::Entry = serde_json::from_value(json!({"id":"plan-model","version":"1.0.0","kind":"model","name":{"en":"Plan model"},"description":{"en":"Plan model"},"config":{"provider":"chatgpt","model_id":"gpt-5.5","endpoint":"https://api.openai.com/v1","plan_connection":"chatgpt","context_window":128000,"max_output_tokens":1024,"modalities":["text"],"cost":{}}})).unwrap();
	let agent: aidash_server::registry::Entry = serde_json::from_value(json!({"id":"plan-agent","version":"1.0.0","kind":"agent","name":{"en":"Plan agent"},"description":{"en":"Plan agent"},"config":{"schema_version":1,"bindings":[],"model":{"id":"plan-model","version":"1.0.0"},"instructions":"Test admission","cluster":null}})).unwrap();
	f.registry
		.register(
			&aidash_domain::tenancy::TenantName::new("alpha").unwrap(),
			model,
		)
		.await
		.unwrap();
	struct Lookup<'a>(&'a aidash_server::registry::Registry);
	#[async_trait]
	impl aidash_application::ports::registry::DefinitionLookup for Lookup<'_> {
		async fn definition(
			&mut self,
			id: &str,
			version: &str,
		) -> Result<aidash_domain::registry::Entry> {
			self.0
				.get(
					&aidash_domain::tenancy::TenantName::new("alpha").unwrap(),
					id,
					version,
				)
				.await
				.map_err(Into::into)
		}
	}
	aidash_application::registry::bindings::resolve(
		&mut aidash_application::registry::bindings::catalog::LocalCatalog(
			aidash_application::registry::bindings::catalog::LookupCatalog {
				definitions: &mut Lookup(&f.registry),
				node: &f.config.node_id,
			},
		),
		&aidash_server::bootstrap::registry_validation().with_plan_connections(true),
		aidash_domain::registry::bindings::QualifiedRef {
			registry_node: f.config.node_id.clone(),
			id: agent.id.clone(),
			version: agent.version.clone(),
		},
		&agent,
		false,
	)
	.await
	.unwrap()
}

#[rstest::rstest]
#[tokio::test]
async fn admission_pins_the_roots_connection_and_refuses_everything_else(
	#[from(parts)] parts: Parts,
	#[future(awt)]
	#[from(plan_application)]
	#[with(parts.clone())]
	application: common::ApplicationFixture,
) {
	let fixture = Fixture::new(application, parts);
	let f = fixture.f.clone();
	let snapshot = snapshot(&f).await;
	let grant = |run: Uuid, subject: &str| aidash_domain::identity::execution::ExecutionGrant {
		run_id: run,
		task_id: Uuid::new_v4(),
		workspace_id: Uuid::new_v4(),
		tenant: "alpha".into(),
		credential_id: Uuid::new_v4(),
		root_subject: subject.into(),
		subject_chain: vec![subject.into()],
	};
	let admit_once = |run: Uuid, subject: &'static str, federated: bool| {
		let f = f.clone();
		let snapshot = snapshot.clone();
		let grant = grant(run, subject);
		async move {
			let mut tx = f
				.store
				.control_pool
				.begin(&TenancyScope::tenant("alpha").unwrap())
				.await
				.unwrap();
			let result = admit(tx.as_mut(), &f.store, &grant, &snapshot, federated).await;
			if result.is_ok() {
				tx.commit().await.unwrap();
			}
			result
		}
	};
	// Without a connection the Run is refused before it starts.
	let run = Uuid::new_v4();
	assert!(matches!(
		admit_once(run, "alice", false).await,
		Err(aidash_server::Error::Plan(Failure::NotConnected))
	));
	let (_, context) = map(&fixture, "browser-a", "alice").await;
	connect(&fixture, "browser-a", &context).await;
	// A task that arrived through Federation never uses one.
	assert!(matches!(
		admit_once(run, "alice", true).await,
		Err(aidash_server::Error::Plan(Failure::NotAdmitted))
	));
	// A chain that starts at another subject cannot use alice's.
	assert!(matches!(
		admit_once(run, "carol", false).await,
		Err(aidash_server::Error::Plan(Failure::NotConnected))
	));
	// Peer admission refuses Plan Connection models outright.
	{
		let mut tx = f
			.store
			.control_pool
			.begin(&TenancyScope::tenant("alpha").unwrap())
			.await
			.unwrap();
		assert!(matches!(
			aidash_server::apps::identity::repositories::provider_credentials::admit(
				tx.as_mut(),
				run,
				"alpha",
				&snapshot,
				false,
			)
			.await,
			Err(aidash_server::Error::Plan(Failure::NotAdmitted))
		));
	}
	admit_once(run, "alice", false).await.unwrap();
	let pin = pinned(&f.store.control_pool, "alpha", run, Provider::Chatgpt)
		.await
		.unwrap();

	// The Run's calls use the owner's access token.
	let access = AdmittedAccess {
		store: f.store.clone(),
	};
	let context = Context {
		tenant: "alpha".into(),
		run: Some(run),
		..Context::default()
	};
	let source = Source::Plan {
		provider: "chatgpt".into(),
	};
	let endpoint = Provider::Chatgpt.base_url();
	access.check(&context, endpoint, &source).await.unwrap();
	let resolved = access.resolve(&context, endpoint, &source).await.unwrap();
	assert_eq!(resolved.bearer.expose_secret(), "plan-access-1");
	// A rejected access token forces one refresh.
	assert!(
		access
			.plan_rejected(
				&context,
				aidash_domain::plan_connections::Disposition::RefreshThenReconnect,
			)
			.await
			.unwrap()
	);
	let resolved = access.resolve(&context, endpoint, &source).await.unwrap();
	assert_eq!(resolved.bearer.expose_secret(), "plan-access-101");
	// Maintenance and other Runs never reach the connection.
	let maintenance = Context {
		tenant: "alpha".into(),
		maintenance: Some(aidash_application::provider_access::MaintenancePurpose::MemoryIndexing),
		..Context::default()
	};
	assert!(matches!(
		access.resolve(&maintenance, endpoint, &source).await,
		Err(Error::Plan(Failure::NotAdmitted))
	));
	let unpinned = Context {
		run: Some(Uuid::new_v4()),
		..context.clone()
	};
	assert!(access.resolve(&unpinned, endpoint, &source).await.is_err());

	// Disconnecting deletes the pin, so the admitted Run fails at its next call.
	let status = json_body(
		&send(
			&fixture,
			"browser-a",
			&context_header(&fixture, "alice").await,
			"GET",
			PLAN,
			Value::Null,
		)
		.await,
	);
	assert_eq!(status["connection"]["id"], pin.connection.to_string());
	let revision = status["connection"]["revision"].clone();
	let disconnected = send(
		&fixture,
		"browser-a",
		&context_header(&fixture, "alice").await,
		"DELETE",
		PLAN,
		json!({"expected_revision": revision}),
	)
	.await;
	assert_eq!(disconnected.status(), 200);
	assert!(matches!(
		access.resolve(&context, endpoint, &source).await,
		Err(Error::Plan(Failure::NotConnected))
	));
	fixture.cleanup().await;
}

/// The dashboard context of the enabled Mapping of `subject` in alpha.
async fn context_header(fixture: &Fixture, subject: &str) -> String {
	let query = Query::select()
		.expr(Expr::cust("id::text"))
		.from(Alias::new("dashboard_mappings"))
		.and_where(Expr::cust(format!(
			"tenant = 'alpha' AND subject = '{subject}' AND enabled"
		)))
		.to_string(PostgresQueryBuilder);
	let id: String = sqlx::query_scalar(&query)
		.fetch_one(fixture.f.store.pool.fixture_driver())
		.await
		.unwrap();
	format!("mapping:alpha:{id}")
}

#[rstest::rstest]
#[tokio::test]
async fn servers_without_a_postgres_store_refuse_to_enable_plan_connections(
	#[future(awt)]
	#[from(common::runtime)]
	runtime: common::RuntimeFixture,
) {
	use aidash_server::apps::identity::serializers::{
		plan_connections::PlanConnectionSettings, provider_credentials::Settings,
	};
	let (mut f, url, schema) = runtime.parts();
	let enabled: PlanConnectionSettings = serde_json::from_value(json!({"enabled": true})).unwrap();
	let disabled: PlanConnectionSettings = serde_json::from_value(json!({})).unwrap();
	assert!(!disabled.enabled);
	let dir = tempfile::tempdir().unwrap();
	let fingerprint = dir.path().join("fingerprint");
	std::fs::write(&fingerprint, "fingerprint-root-at-least-32-bytes-159").unwrap();
	let none: Settings = serde_json::from_value(json!({})).unwrap();
	let cloud: Settings = serde_json::from_value(json!({"fingerprint_key":{"file":fingerprint},"store":{"kind":"secret_manager","byok_project_id":"fixture-byok","environment_id":"local"}})).unwrap();
	for settings in [&none, &cloud] {
		let error =
			aidash_server::bootstrap::configure_plan_connections(&mut f.store, settings, &enabled)
				.unwrap_err();
		assert!(
			error
				.to_string()
				.contains("PostgreSQL Provider Credential Store")
		);
		aidash_server::bootstrap::configure_plan_connections(&mut f.store, settings, &disabled)
			.unwrap();
		assert!(f.store.plan_connections.is_none());
		assert!(!f.store.plan_connections_enabled);
	}
	common::cleanup(f, &url, &schema).await;
}
