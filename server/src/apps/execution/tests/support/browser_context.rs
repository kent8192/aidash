//! Browser contexts for #484 regressions and Tenant administration fixtures.
//!
//! The shared `api_token` bearer is one Deployment Administrator; a browser
//! session acting in the `deployment_administrator` context is the other. Both
//! must be refused on every Tenant content route. Tenant administration
//! (credentials, Nodes, Node registrations, Peer Mappings) is only open to a
//! Tenant Administrator acting through a browser Mapping, which
//! [`BrowserMapping`] provides.
use aidash_server::{config::OidcConfig, federation::Federation};
use reinhardt::InjectionContext;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder};
use reinhardt::test::APIClient;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

/// The digest a current-format browser session stores.
fn session_digest(value: &[u8]) -> Vec<u8> {
	let mut digest = Sha256::new();
	digest.update(b"aidash-browser-session-v2\0");
	digest.update(value);
	digest.finalize().to_vec()
}

const ORIGIN: &str = "https://localhost:8080";

/// Seeds a browser identity with a live session and turns the application's
/// OIDC on (the fixture's Federation is replaced in `context`). Returns the
/// identity and the session secrets.
async fn browser(f: &Federation, context: &InjectionContext) -> (Uuid, String, String) {
	let identity = Uuid::new_v4();
	let session = format!("browser-session-{identity}");
	let csrf = format!("browser-csrf-{identity}");
	let queries = [
		Query::insert()
			.into_table(Alias::new("dashboard_identities"))
			.columns(["id", "issuer", "subject", "last_valid_at"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::val(identity))
					.expr(Expr::val("https://accounts.google.com"))
					.expr(Expr::val(format!("browser-{identity}")))
					.expr(Expr::cust("clock_timestamp()"))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
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
					.expr(Expr::val(session_digest(session.as_bytes())))
					.expr(Expr::val(Sha256::digest(csrf.as_bytes()).to_vec()))
					.expr(Expr::cust("clock_timestamp()"))
					.expr(Expr::cust("clock_timestamp()"))
					.expr(Expr::cust("clock_timestamp()+interval '12 hours'"))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	];
	for query in queries {
		sqlx::query(&query)
			.execute(f.store.pool.fixture_driver())
			.await
			.expect("the browser identity is seeded");
	}
	// Keep whatever the fixture already configured, such as a Deployment Key.
	let current = context
		.get_singleton::<Federation>()
		.map_or_else(|| f.clone(), |current| (*current).clone());
	context.set_singleton(configured(&current));
	(identity, session, csrf)
}

/// `f` with browser sign-in configured.
#[allow(dead_code)] // Application-building fixtures configure the Federation first.
pub fn configured(f: &Federation) -> Federation {
	let mut configured = f.clone();
	configured.config.oidc = Some(OidcConfig {
		issuer: "https://accounts.google.com".into(),
		client_id: "aidash".into(),
		client_secret: "local-test-secret".into(),
		public_origin: ORIGIN.into(),
		session_absolute_seconds: 43_200,
		session_idle_seconds: 1_800,
	});
	configured
}

async fn send_as(
	client: &APIClient,
	headers: &[(&str, String)],
	method: &str,
	path: &str,
	body: Value,
) -> (u16, Value) {
	let mut request = client.request(method.parse().unwrap(), path);
	for (name, value) in headers {
		request = request.header(*name, value.as_str());
	}
	if matches!(method, "POST" | "PUT" | "PATCH") {
		request = request.json(&body);
	}
	let response = request.send().await.unwrap();
	let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
	(response.status_code(), parsed)
}

fn headers(session: &str, csrf: &str, context: &str) -> Vec<(&'static str, String)> {
	vec![
		("cookie", format!("__Host-aidash-session={session}")),
		("origin", ORIGIN.into()),
		("x-aidash-csrf", csrf.into()),
		("x-aidash-context", context.into()),
	]
}

/// A browser session of an identity acting as `subject` of `tenant` through a
/// Mapping: the Tenant Administrator context that administers credentials,
/// Nodes, Node registrations and Peer Mappings (#484). The Tenant's policy must
/// define and enable `subject` and allow it the administration it exercises.
/// Browser sign-in is turned on in `context` for the rest of the fixture.
#[allow(dead_code)] // Only suites that administer a Tenant over HTTP build one.
pub struct BrowserMapping {
	client: Arc<APIClient>,
	/// The id of the credential the Mapping is bound to.
	pub credential: String,
	headers: Vec<(&'static str, String)>,
}

#[allow(dead_code)]
impl BrowserMapping {
	pub async fn on(
		f: &Federation,
		context: &InjectionContext,
		client: Arc<APIClient>,
		tenant: &str,
		subject: &str,
	) -> Self {
		let credential = super::tenant_roots::issued_credential(&f.store, tenant, subject).await
			["credential"]["id"]
			.as_str()
			.expect("the Mapping's credential id")
			.to_owned();
		let (identity, session, csrf) = browser(f, context).await;
		let mapping = Uuid::new_v4();
		let insert = Query::insert()
			.into_table(Alias::new("dashboard_mappings"))
			.columns(["id", "identity_id", "tenant", "subject", "credential_id"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::val(mapping))
					.expr(Expr::val(identity))
					.expr(Expr::val(tenant))
					.expr(Expr::val(subject))
					.expr(Expr::cust(format!("'{credential}'::uuid")))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder);
		sqlx::query(&insert)
			.execute(f.store.pool.fixture_driver())
			.await
			.expect("the Mapping is seeded");
		Self {
			client,
			credential,
			headers: headers(&session, &csrf, &format!("mapping:{tenant}:{mapping}")),
		}
	}

	/// The response status and JSON body (null when the body is not JSON).
	pub async fn send(&self, method: &str, path: &str, body: Value) -> (u16, Value) {
		send_as(&self.client, &self.headers, method, path, body).await
	}
}

/// A browser session of an identity holding a Deployment Administrator grant,
/// acting in the `deployment_administrator` context. Browser sign-in is turned
/// on in `context` for the rest of the fixture.
#[allow(dead_code)] // Only refusal regressions build a browser administrator.
pub struct BrowserAdministrator {
	client: Arc<APIClient>,
	headers: Vec<(&'static str, String)>,
}

#[allow(dead_code)]
impl BrowserAdministrator {
	pub async fn on(f: &Federation, context: &InjectionContext, client: Arc<APIClient>) -> Self {
		let (identity, session, csrf) = browser(f, context).await;
		let grant = Query::insert()
			.into_table(Alias::new("dashboard_deployment_administrator_grants"))
			.columns([Alias::new("identity_id")])
			.from_subquery(Query::select().expr(Expr::val(identity)).to_owned())
			.to_string(PostgresQueryBuilder);
		sqlx::query(&grant)
			.execute(f.store.pool.fixture_driver())
			.await
			.expect("the Deployment Administrator grant is seeded");
		Self {
			client,
			headers: headers(&session, &csrf, "deployment_administrator"),
		}
	}

	/// The response status and JSON body (null when the body is not JSON).
	pub async fn send(&self, method: &str, path: &str, body: Value) -> (u16, Value) {
		send_as(&self.client, &self.headers, method, path, body).await
	}

	/// The request headers of this browser context, for SSE reads.
	pub fn headers(&self) -> Vec<(&'static str, String)> {
		self.headers.clone()
	}
}
