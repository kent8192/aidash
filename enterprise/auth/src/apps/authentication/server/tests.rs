//! The downstream issuer and BFF contract over actual HTTP and PostgreSQL.
//! GCIP verification owns its existing signed-token tests. These fixtures seed
//! an already-verified source through the production repository and use the real
//! account/session, OIDC, OAuth, persistence and HTTP implementations thereafter.
use crate::{
	config::{Client, Config, Grpc},
	service::{Authentication, digest, secret},
};
use aidash_application::{accounts::Service as Accounts, ports::accounts::Scope};
use aidash_domain::accounts::*;
use base64::{
	Engine as _,
	engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chrono::Utc;
use reinhardt::http::Handler;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use uuid::Uuid;

const CLIENT_SECRET: &str = "fixture-client-secret-at-least-32-characters+/=";
const RESOURCE_SECRET: &str = "fixture-resource-secret-at-least-32-characters";
const VERIFIER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
fn client_basic_secret() -> String {
	url::form_urlencoded::byte_serialize(CLIENT_SECRET.as_bytes()).collect()
}
fn with_login(url: &str, user: &str, password: &str) -> String {
	let mut url = url::Url::parse(url).unwrap();
	url.set_username(user).unwrap();
	url.set_password(Some(password)).unwrap();
	url.to_string()
}
fn spawn(
	listener: tokio::net::TcpListener,
	handler: impl Handler + 'static,
) -> tokio::task::JoinHandle<()> {
	tokio::spawn(async move {
		reinhardt::server::HttpServer::new(handler)
			.listen_on_with_shutdown(
				listener,
				reinhardt::server::ShutdownCoordinator::new(Duration::from_secs(2)),
			)
			.await
			.unwrap();
	})
}
async fn ready(client: &reqwest::Client, origin: &str, path: &str) {
	for _ in 0..100 {
		if client.get(format!("{origin}{path}")).send().await.is_ok() {
			return;
		}
		tokio::time::sleep(Duration::from_millis(10)).await;
	}
	panic!("listener did not start")
}
fn cookie(response: &reqwest::Response, name: &str) -> String {
	response
		.headers()
		.get_all("set-cookie")
		.iter()
		.filter_map(|v| v.to_str().ok())
		.find_map(|v| {
			v.split(';')
				.next()
				.filter(|v| v.starts_with(&format!("{name}=")))
		})
		.unwrap_or_else(|| panic!("missing {name} cookie"))
		.to_owned()
}
fn form(client: &reqwest::Client, url: String, fields: &[(&str, &str)]) -> reqwest::RequestBuilder {
	let body = url::form_urlencoded::Serializer::new(String::new())
		.extend_pairs(fields.iter().copied())
		.finish();
	client
		.post(url)
		.header("content-type", "application/x-www-form-urlencoded")
		.body(body)
}
async fn authorize(
	client: &reqwest::Client,
	issuer: &str,
	callback: &str,
	browser: &str,
	nonce: &str,
) -> String {
	let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()));
	let mut url = url::Url::parse(&format!("{issuer}/oauth/authorize")).unwrap();
	url.query_pairs_mut().extend_pairs([
		("response_type", "code"),
		("client_id", "marketplace"),
		("redirect_uri", callback),
		("scope", "openid"),
		("state", "fixture-state"),
		("nonce", nonce),
		("code_challenge", challenge.as_str()),
		("code_challenge_method", "S256"),
	]);
	let response = client
		.get(url)
		.header("cookie", format!("aidash-auth={browser}"))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 302, "{}", response.text().await.unwrap());
	let location = response.headers()["location"].to_str().unwrap();
	let url = url::Url::parse(location).unwrap();
	assert_eq!(
		url.query_pairs().find(|(key, _)| key == "state").unwrap().1,
		"fixture-state"
	);
	url.query_pairs()
		.find(|(key, _)| key == "code")
		.unwrap_or_else(|| panic!("authorization did not issue a code: {location}"))
		.1
		.into_owned()
}
async fn exchange(client: &reqwest::Client, issuer: &str, callback: &str, code: &str) -> Value {
	let response = form(
		client,
		format!("{issuer}/oauth/token"),
		&[
			("grant_type", "authorization_code"),
			("code", code),
			("redirect_uri", callback),
			("code_verifier", VERIFIER),
		],
	)
	.basic_auth("marketplace", Some(&client_basic_secret()))
	.send()
	.await
	.unwrap();
	assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
	response.json().await.unwrap()
}
async fn introspect(client: &reqwest::Client, issuer: &str, token: &str) -> Value {
	let response = form(
		client,
		format!("{issuer}/oauth/introspect"),
		&[("token", token)],
	)
	.basic_auth("marketplace-resource", Some(RESOURCE_SECRET))
	.send()
	.await
	.unwrap();
	assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
	response.json().await.unwrap()
}

pub(super) struct Fixture {
	pub(super) auth: Arc<Authentication>,
	pub(super) client: reqwest::Client,
	pub(super) issuer: String,
	pub(super) marketplace_origin: String,
	pub(super) callback: String,
	pub(super) account: Account,
	pub(super) identity: ExternalIdentity,
	pub(super) browser: String,
	admin: reinhardt::db::backends::DatabaseConnection,
	marketplace_database_url: String,
	owner_config: Config,
	tasks: Vec<tokio::task::JoinHandle<()>>,
	_guards: Vec<Box<dyn std::any::Any + Send>>,
}
impl Drop for Fixture {
	fn drop(&mut self) {
		for task in &self.tasks {
			task.abort();
		}
	}
}
// The fixture administrator provisions the services' isolated roles and schemas;
// it never composes Cloud's scoped pool or supplies a serving credential.
#[allow(clippy::disallowed_methods)]
pub(super) async fn fixture(configure: impl FnOnce(&mut Authentication)) -> Fixture {
	let (_container, _pool, _port, admin_url) = reinhardt::test::fixtures::postgres_container_with(
		reinhardt::test::fixtures::PostgresContainerConfig::default()
			.image("postgres", "17-alpine")
			.user("postgres")
			.password("fixture-admin-password")
			.database("authentication"),
	)
	.await;
	let admin = reinhardt::db::backends::DatabaseConnection::connect_postgres(&admin_url)
		.await
		.unwrap();
	// Infrastructure-only DDL provisions isolated roles; application reads/writes
	// below go exclusively through the owning service's real repository/ORM.
	for ddl in include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/bootstrap.sql"))
		.lines()
		.filter(|line| !line.trim_start().starts_with("--"))
		.collect::<Vec<_>>()
		.join("\n")
		.split(';')
		.map(str::trim)
		.filter(|ddl| !ddl.is_empty())
	{
		admin.execute(ddl, Vec::new()).await.unwrap();
	}
	for ddl in include_str!(concat!(
		env!("CARGO_MANIFEST_DIR"),
		"/../marketplace/bootstrap.sql"
	))
	.lines()
	.filter(|line| !line.trim_start().starts_with("--"))
	.collect::<Vec<_>>()
	.join("\n")
	.split(';')
	.map(str::trim)
	.filter(|ddl| !ddl.is_empty())
	{
		admin.execute(ddl, Vec::new()).await.unwrap();
	}
	for sql in [
		"ALTER ROLE aidash_auth_owner LOGIN PASSWORD 'fixture-auth-owner'",
		"ALTER ROLE aidash_auth_app LOGIN PASSWORD 'fixture-auth-runtime'",
		"ALTER ROLE marketplace_owner LOGIN PASSWORD 'fixture-marketplace-owner'",
		"ALTER ROLE marketplace_app LOGIN PASSWORD 'fixture-marketplace-runtime'",
		"REVOKE ALL ON SCHEMA public FROM PUBLIC",
	] {
		admin.execute(sql, Vec::new()).await.unwrap();
	}

	let temp = reinhardt::test::fixtures::temp_dir();
	let write = |name: &str, text: &str| {
		let path = temp.path().join(name);
		std::fs::write(&path, text).unwrap();
		path
	};
	let auth_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let marketplace_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let issuer = format!("http://{}", auth_listener.local_addr().unwrap());
	let marketplace_origin = format!("http://{}", marketplace_listener.local_addr().unwrap());
	let callback = format!("{marketplace_origin}/auth/callback");
	let owner_url = with_login(&admin_url, "aidash_auth_owner", "fixture-auth-owner");
	let runtime_url = with_login(&admin_url, "aidash_auth_app", "fixture-auth-runtime");
	let owner_file = write("owner.url", &owner_url);
	let runtime_file = write("runtime.url", &runtime_url);
	let client_secret = write("client.secret", CLIENT_SECRET);
	let resource_secret = write("resource.secret", RESOURCE_SECRET);
	let key = write(
		"test-only-signing.pem",
		include_str!(concat!(
			env!("CARGO_MANIFEST_DIR"),
			"/../crates/aidash-gcp/src/gcip/signing-test-only.pem"
		)),
	);
	let encryption = write("encryption.secret", &STANDARD.encode([73; 32]));
	let mut config = Config {
		issuer: issuer.clone(),
		listen: auth_listener.local_addr().unwrap(),
		database_url_file: owner_file,
		schema: "aidash_auth".into(),
		gcip_project: "fixture-project".into(),
		gcip_tenants: vec!["shared".into()],
		gcip_providers: BTreeMap::new(),
		gcip_password_sign_up: false,
		gcip_mfa_mandatory: vec![],
		trusted_proxy_ips: vec![],
		assets_dir: None,
		gcip_browser_api_key: "fixture-public-browser-key".into(),
		gcip_auth_domain: "fixture-project.firebaseapp.com".into(),
		signing_key_file: key,
		token_encryption_key_file: encryption,
		signing_key_id: "fixture-signing-key".into(),
		loopback_development: true,
		clients: vec![
			Client {
				id: "marketplace".into(),
				redirect_uri: callback.clone(),
				secret_file: client_secret.clone(),
				introspection_secret_file: resource_secret.clone(),
			},
			Client {
				id: "cloud".into(),
				redirect_uri: "http://127.0.0.1:9871/callback".into(),
				secret_file: client_secret,
				introspection_secret_file: resource_secret,
			},
		],
		grpc: Grpc {
			listen: "127.0.0.1:0".parse().unwrap(),
			certificate_file: "unused-fixture-cert".into(),
			private_key_file: "unused-fixture-key".into(),
			client_ca_file: "unused-fixture-ca".into(),
			product_peers: [("cloud".into(), "spiffe://aidash/cloud".into())].into(),
			administrator_peers: vec!["spiffe://aidash/auth-admin".into()],
			source_restore_clients: vec![],
		},
	};
	let owner = Authentication::connect(config.clone()).await.unwrap();
	owner.migrate().await.unwrap();
	Authentication::provision(&config, &owner).await.unwrap();
	owner
		.connection
		.execute(
			include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/seal-ledger.sql")),
			Vec::new(),
		)
		.await
		.unwrap();
	let owner_config = config.clone();
	config.database_url_file = runtime_file;
	let store = Authentication::connect(config.clone()).await.unwrap();
	let mut service = Authentication::new(config, store).await.unwrap();
	configure(&mut service);
	let auth = Arc::new(service);
	let account = Account {
		id: Uuid::new_v4(),
		status: AccountStatus::Active,
		session_epoch: 0,
		suspension_epoch: 0,
	};
	let identity = ExternalIdentity {
		id: Uuid::new_v4(),
		issuer: "https://securetoken.google.com/fixture-project/tenants/shared".into(),
		subject: "verified-upstream-subject".into(),
		first_account: Some(account.id),
		linked: true,
		status: IdentityStatus::Enabled,
		disable_epoch: 0,
		last_valid_at: Some(Utc::now()),
	};
	let mut tx = auth.store.scope().await.unwrap();
	tx.save_account(&account).await.unwrap();
	tx.save_identity(&identity).await.unwrap();
	Box::new(tx).commit().await.unwrap();
	let sso = auth
		.issue_session(identity.id, "sso", Utc::now())
		.await
		.unwrap();
	let browser = secret();
	let mut tx = auth.store.scope().await.unwrap();
	tx.bind(format!("sso:{}", digest(&browser)), sso.id, "sso")
		.await
		.unwrap();
	Box::new(tx).commit().await.unwrap();
	let handler = crate::config::urls::handler(auth.clone());
	#[cfg(feature = "browser-e2e")]
	let handler = browser_e2e::BrowserUpstream(handler);
	let auth_task = spawn(auth_listener, handler);
	let client = reqwest::Client::builder()
		.redirect(reqwest::redirect::Policy::none())
		.timeout(Duration::from_secs(15))
		.build()
		.unwrap();
	ready(&client, &issuer, "/.well-known/openid-configuration").await;
	let metadata: Value = client
		.get(format!("{issuer}/.well-known/openid-configuration"))
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(metadata["issuer"], issuer);

	// Actual Marketplace BFF login: discovery, PKCE, signed ID token + nonce
	// verification, online introspection and product-local opaque cookie storage.
	let marketplace_owner =
		with_login(&admin_url, "marketplace_owner", "fixture-marketplace-owner");
	let marketplace_runtime =
		with_login(&admin_url, "marketplace_app", "fixture-marketplace-runtime");
	aidash_marketplace_server::migrations::migrate(&marketplace_owner)
		.await
		.unwrap();
	let marketplace = aidash_marketplace_server::Marketplace::connect(
		aidash_marketplace_server::Config {
			assets_dir: if cfg!(feature = "browser-e2e") {
				Some(
					std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
						.join("../marketplace/dist"),
				)
			} else {
				None
			},
			origin: marketplace_origin.clone(),
			issuer: issuer.clone(),
			trusted_proxy_ips: vec![],
			client_id: "marketplace".into(),
			client_secret: CLIENT_SECRET.into(),
			resource_client_id: "marketplace-resource".into(),
			resource_secret: RESOURCE_SECRET.into(),
			billing_secret: secret(),
			enrollment_secret: secret(),
			recall_secret: secret(),
		},
		&marketplace_runtime,
	)
	.await
	.unwrap();
	let marketplace_task = spawn(
		marketplace_listener,
		aidash_marketplace_server::config::urls::handler(marketplace.clone()),
	);
	ready(&client, &marketplace_origin, "/healthz").await;

	Fixture {
		auth,
		client,
		issuer,
		marketplace_origin,
		callback,
		account,
		identity,
		browser,
		admin,
		marketplace_database_url: marketplace_runtime,
		owner_config,
		tasks: vec![auth_task, marketplace_task],
		_guards: vec![Box::new(_container), Box::new(temp)],
	}
}

#[tokio::test]
async fn migration_retry_preserves_the_active_signing_key_and_rejects_key_id_reuse() {
	use reinhardt::auth::oidc_op::{OidcStateStore, PostgresOidcStore};
	let fixture = fixture(|_| {}).await;
	let owner = Authentication::connect(fixture.owner_config.clone())
		.await
		.unwrap();
	let keys = PostgresOidcStore::new(owner.pool.clone());
	let before = keys.active_key().await.unwrap().unwrap();
	owner.migrate().await.unwrap();
	Authentication::provision(&fixture.owner_config, &owner)
		.await
		.unwrap();
	let after = keys.active_key().await.unwrap().unwrap();
	assert_eq!(after.public, before.public);
	assert_eq!(after.activated_at, before.activated_at);
	assert!(after.active && !after.compromised);
	let directory = reinhardt::test::fixtures::temp_dir();
	let changed_key = directory.path().join("different-key.pem");
	assert!(
		std::process::Command::new("openssl")
			.args(["genrsa", "-out"])
			.arg(&changed_key)
			.arg("2048")
			.output()
			.unwrap()
			.status
			.success()
	);
	let changed = Config {
		signing_key_file: changed_key,
		..fixture.owner_config.clone()
	};
	let replacement = reinhardt::auth::oidc_op::RsaPemKeyRing::new()
		.add_private_key_pem(
			&changed.signing_key_id,
			&std::fs::read_to_string(&changed.signing_key_file).unwrap(),
		)
		.unwrap();
	assert_ne!(replacement.n, before.public.n);
	assert!(matches!(
		Authentication::provision(&changed, &owner).await,
		Err(crate::Error::Configuration)
	));
	assert_eq!(
		keys.active_key().await.unwrap().unwrap().public,
		before.public
	);
}

#[tokio::test]
async fn refresh_lock_contention_keeps_lifecycle_connections_available() {
	let (_container, _pool, _port, url) = reinhardt::test::fixtures::postgres_container_with(
		reinhardt::test::fixtures::PostgresContainerConfig::default()
			.image("postgres", "17-alpine"),
	)
	.await;
	let pool = sqlx::postgres::PgPoolOptions::new()
		.max_connections(1)
		.acquire_timeout(Duration::from_millis(250))
		.connect(&url)
		.await
		.unwrap();
	let waiting_store = crate::store::Store::from_pool(pool.clone());
	let store = crate::store::Store::from_pool(pool);
	let first = store.refresh_lock("same-refresh-family").await.unwrap();
	let mut waiter = tokio::spawn(async move {
		waiting_store
			.refresh_lock("same-refresh-family")
			.await
			.unwrap()
	});
	// Even a minimal lifecycle pool must remain usable while refresh callers
	// retain or wait for the family lock on their separate pool.
	let scope = store
		.scope()
		.await
		.expect("refresh lock must not consume lifecycle capacity");
	drop(scope);
	assert!(
		tokio::time::timeout(Duration::from_millis(100), &mut waiter)
			.await
			.is_err()
	);
	drop(first);
	let second = tokio::time::timeout(Duration::from_secs(2), waiter)
		.await
		.expect("the next refresh proceeds after the holder rolls back")
		.unwrap();
	second.commit().await.unwrap();
}

#[tokio::test]
async fn oidc_issuer_and_marketplace_bff_preserve_real_session_and_refresh_boundaries() {
	let assets = reinhardt::test::fixtures::temp_dir();
	std::fs::write(
		assets.path().join("index.html"),
		"<!doctype html><title>Account</title>",
	)
	.unwrap();
	let fixture = fixture(|auth| {
		let config = Arc::make_mut(&mut auth.config);
		config.assets_dir = Some(assets.path().to_owned());
		config.gcip_auth_domain = "login.example.com".into();
	})
	.await;
	let auth = fixture.auth.clone();
	let client = fixture.client.clone();
	let issuer = fixture.issuer.clone();
	let marketplace_origin = fixture.marketplace_origin.clone();
	let callback = fixture.callback.clone();
	let account = fixture.account.clone();
	let identity = fixture.identity.clone();
	let browser = fixture.browser.clone();
	let page = client
		.get(format!("{issuer}/sign-in"))
		.send()
		.await
		.unwrap();
	assert_eq!(page.status(), 200);
	let csp = page.headers()["content-security-policy"].to_str().unwrap();
	assert!(csp.split(';').any(|directive| {
		let mut tokens = directive.split_whitespace();
		tokens.next() == Some("script-src") && tokens.any(|token| token == "'wasm-unsafe-eval'")
	}));
	assert!(!csp.split_whitespace().any(|token| token == "'unsafe-eval'"));
	let frames: Vec<_> = csp
		.split(';')
		.find_map(|directive| directive.trim().strip_prefix("frame-src "))
		.unwrap()
		.split_whitespace()
		.collect();
	assert!(frames.contains(&"https://login.example.com"));
	assert!(frames.contains(&"https://*.firebaseapp.com"));
	assert!(frames.contains(&"https://accounts.google.com"));
	let directive = |name: &str| {
		csp.split(';')
			.find_map(|directive| directive.trim().strip_prefix(name))
			.map(|tokens| tokens.split_whitespace().collect::<Vec<_>>())
	};
	// `<style>` elements stay blocked; only inline style attributes (Firebase's
	// hidden gapi iframe) are allowed.
	assert_eq!(directive("style-src ").unwrap(), ["'self'"]);
	assert_eq!(directive("style-src-attr ").unwrap(), ["'unsafe-inline'"]);
	assert_eq!(directive("object-src ").unwrap(), ["'none'"]);
	// Loopback development is plain HTTP, where HSTS would be meaningless.
	assert!(!page.headers().contains_key("strict-transport-security"));

	// Outside loopback development every response carries HSTS, and plain HTTP
	// is refused unless a trusted proxy asserts HTTPS.
	let mut secure = (*auth).clone();
	{
		let config = Arc::make_mut(&mut secure.config);
		config.loopback_development = false;
		config.trusted_proxy_ips = vec!["127.0.0.1".parse().unwrap()];
	}
	let secure_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let secure_origin = format!("http://{}", secure_listener.local_addr().unwrap());
	let secure_task = spawn(
		secure_listener,
		crate::config::urls::handler(Arc::new(secure)),
	);
	ready(&client, &secure_origin, "/healthz").await;
	let hsts = "max-age=63072000; includeSubDomains";
	for (path, forwarded, status) in [
		("/healthz", false, 200),
		("/sign-in", false, 400),
		("/sign-in", true, 200),
	] {
		let mut request = client.get(format!("{secure_origin}{path}"));
		if forwarded {
			request = request.header("x-forwarded-proto", "https");
		}
		let response = request.send().await.unwrap();
		assert_eq!(response.status(), status, "{path} forwarded={forwarded}");
		assert_eq!(response.headers()["strict-transport-security"], hsts);
	}
	let rpc = client
		.post(format!("{secure_origin}/api/server_fn/auth_account"))
		.send()
		.await
		.unwrap();
	assert_eq!(rpc.headers()["strict-transport-security"], hsts);
	secure_task.abort();
	// A CIDR network trusts forwarded HTTPS from any peer inside it and no other.
	for (proxy, status) in [("127.0.0.0/8", 200), ("10.0.0.0/8", 400)] {
		let mut networked = (*auth).clone();
		{
			let config = Arc::make_mut(&mut networked.config);
			config.loopback_development = false;
			config.trusted_proxy_ips = vec![proxy.parse().unwrap()];
		}
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let origin = format!("http://{}", listener.local_addr().unwrap());
		let task = spawn(listener, crate::config::urls::handler(Arc::new(networked)));
		ready(&client, &origin, "/healthz").await;
		let response = client
			.get(format!("{origin}/sign-in"))
			.header("x-forwarded-proto", "https")
			.send()
			.await
			.unwrap();
		assert_eq!(response.status(), status, "{proxy}");
		task.abort();
	}
	let nonce = secret();
	let code = authorize(&client, &issuer, &callback, &browser, &nonce).await;
	let wrong_client = form(
		&client,
		format!("{issuer}/oauth/token"),
		&[
			("grant_type", "authorization_code"),
			("code", &code),
			("redirect_uri", &callback),
			("code_verifier", VERIFIER),
		],
	)
	.basic_auth("cloud", Some(&client_basic_secret()))
	.send()
	.await
	.unwrap();
	assert_eq!(wrong_client.status(), 401);
	let tokens = exchange(&client, &issuer, &callback, &code).await;
	let replay_code = authorize(&client, &issuer, &callback, &browser, &secret()).await;
	let replay_tokens = exchange(&client, &issuer, &callback, &replay_code).await;
	assert_eq!(
		form(
			&client,
			format!("{issuer}/oauth/token"),
			&[
				("grant_type", "authorization_code"),
				("code", &replay_code),
				("redirect_uri", &callback),
				("code_verifier", VERIFIER)
			]
		)
		.basic_auth("marketplace", Some(&client_basic_secret()))
		.send()
		.await
		.unwrap()
		.status(),
		400,
		"a code is single-use"
	);
	assert_eq!(
		introspect(
			&client,
			&issuer,
			replay_tokens["access_token"].as_str().unwrap()
		)
		.await["active"],
		false,
		"reusing a code revokes that grant without invalidating an independent grant"
	);
	let id_payload = URL_SAFE_NO_PAD
		.decode(
			tokens["id_token"]
				.as_str()
				.unwrap()
				.split('.')
				.nth(1)
				.unwrap(),
		)
		.unwrap();
	let id_payload: Value = serde_json::from_slice(&id_payload).unwrap();
	assert_eq!(id_payload["nonce"], nonce);
	assert!(
		id_payload["sub"]
			.as_str()
			.is_some_and(|sub| !sub.is_empty())
	);
	let access = tokens["access_token"].as_str().unwrap();
	let claims = introspect(&client, &issuer, access).await;
	assert_eq!(claims["active"], true);
	assert_eq!(claims["sub"], id_payload["sub"]);
	assert_eq!(claims["account_id"], account.id.to_string());
	assert_eq!(claims["source_identity_id"], identity.id.to_string());
	let userinfo: Value = client
		.get(format!("{issuer}/oauth/userinfo"))
		.bearer_auth(access)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(userinfo["sub"], id_payload["sub"]);
	assert_eq!(userinfo["account_id"], account.id.to_string());
	let cross = form(
		&client,
		format!("{issuer}/oauth/introspect"),
		&[("token", access)],
	)
	.basic_auth("cloud-resource", Some(RESOURCE_SECRET))
	.send()
	.await
	.unwrap()
	.json::<Value>()
	.await
	.unwrap();
	assert_eq!(
		cross["active"], false,
		"a product cannot introspect a different client's token"
	);
	assert_eq!(
		form(
			&client,
			format!("{issuer}/oauth/introspect"),
			&[("token", access)]
		)
		.basic_auth("marketplace", Some(&client_basic_secret()))
		.send()
		.await
		.unwrap()
		.status(),
		401,
		"OAuth client credentials do not grant resource-server authority"
	);

	let login = client
		.get(format!("{marketplace_origin}/auth/login"))
		.send()
		.await
		.unwrap();
	assert_eq!(login.status(), 302, "{}", login.text().await.unwrap());
	let login_cookie = cookie(&login, "marketplace-login");
	let authorization = client
		.get(login.headers()["location"].to_str().unwrap())
		.header("cookie", format!("aidash-auth={browser}"))
		.send()
		.await
		.unwrap();
	assert_eq!(
		authorization.status(),
		302,
		"{}",
		authorization.text().await.unwrap()
	);
	let location = authorization.headers()["location"]
		.to_str()
		.unwrap()
		.to_owned();
	let mut wrong = url::Url::parse(&location).unwrap();
	let code = wrong
		.query_pairs()
		.find(|(key, _)| key == "code")
		.unwrap()
		.1
		.into_owned();
	wrong.set_query(Some(
		&url::form_urlencoded::Serializer::new(String::new())
			.append_pair("state", "wrong-browser-state")
			.append_pair("code", &code)
			.finish(),
	));
	assert_eq!(
		client
			.get(wrong)
			.header("cookie", &login_cookie)
			.send()
			.await
			.unwrap()
			.status(),
		401
	);
	let callback_response = client
		.get(&location)
		.header("cookie", &login_cookie)
		.send()
		.await
		.unwrap();
	assert_eq!(
		callback_response.status(),
		302,
		"{}",
		callback_response.text().await.unwrap()
	);
	let product_cookie = cookie(&callback_response, "marketplace-session");
	assert_eq!(
		client
			.get(&location)
			.header("cookie", &login_cookie)
			.send()
			.await
			.unwrap()
			.status(),
		401,
		"authorization callback cannot be replayed"
	);
	let session: Value = client
		.get(format!("{marketplace_origin}/api/marketplace/session"))
		.header("cookie", &product_cookie)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(session["authenticated"], true);
	assert_eq!(session["account_id"], account.id.to_string());
	for secret in ["access_token", "refresh_token", "id_token"] {
		assert!(
			session.get(secret).is_none(),
			"BFF must not expose {secret}"
		);
	}

	// Safe response-loss retry returns the exact response. Another request ID
	// reusing the same old token is a definite family-local revocation.
	let refresh = tokens["refresh_token"].as_str().unwrap();
	let request_id = Uuid::new_v4().to_string();
	let refresh_request = |id: &str| {
		form(
			&client,
			format!("{issuer}/oauth/token"),
			&[("grant_type", "refresh_token"), ("refresh_token", refresh)],
		)
		.basic_auth("marketplace", Some(&client_basic_secret()))
		.header("idempotency-key", id)
	};
	let rotated = refresh_request(&request_id).send().await.unwrap();
	assert_eq!(rotated.status(), 200, "{}", rotated.text().await.unwrap());
	let rotated: Value = rotated.json().await.unwrap();
	let (first_retry, second_retry) = tokio::join!(
		refresh_request(&request_id).send(),
		refresh_request(&request_id).send(),
	);
	for retry in [first_retry.unwrap(), second_retry.unwrap()] {
		assert_eq!(retry.status(), 200);
		assert_eq!(retry.json::<Value>().await.unwrap(), rotated);
	}
	assert!(
		!refresh_request(&Uuid::new_v4().to_string())
			.send()
			.await
			.unwrap()
			.status()
			.is_success()
	);
	assert_eq!(
		introspect(&client, &issuer, rotated["access_token"].as_str().unwrap()).await["active"],
		false
	);
	assert_eq!(
		client
			.get(format!("{marketplace_origin}/api/marketplace/library"))
			.header("cookie", &product_cookie)
			.send()
			.await
			.unwrap()
			.status(),
		200,
		"replay revocation must not log out another product/device family"
	);

	// A second linked source remains independently usable. Disabling the
	// original source must not borrow that sibling's authority for the BFF.
	let mut sibling = identity.clone();
	sibling.id = Uuid::new_v4();
	sibling.subject = "other-verified-upstream-subject".into();
	let mut tx = auth.store.scope().await.unwrap();
	tx.save_identity(&sibling).await.unwrap();
	Box::new(tx).commit().await.unwrap();
	let sibling_sso = auth
		.issue_session(sibling.id, "sso", Utc::now())
		.await
		.unwrap();
	let sibling_browser = secret();
	let mut tx = auth.store.scope().await.unwrap();
	tx.bind(
		format!("sso:{}", digest(&sibling_browser)),
		sibling_sso.id,
		"sso",
	)
	.await
	.unwrap();
	Box::new(tx).commit().await.unwrap();
	let sibling_code = authorize(&client, &issuer, &callback, &sibling_browser, &secret()).await;
	let sibling_tokens = exchange(&client, &issuer, &callback, &sibling_code).await;
	let mut disabled = identity.clone();
	disabled.status = IdentityStatus::Disabled;
	let mut tx = auth.store.scope().await.unwrap();
	tx.save_identity(&disabled).await.unwrap();
	tx.revoke_identity_sessions(identity.id, Utc::now())
		.await
		.unwrap();
	Box::new(tx).commit().await.unwrap();
	assert_eq!(
		client
			.get(format!("{marketplace_origin}/api/marketplace/library"))
			.header("cookie", &product_cookie)
			.send()
			.await
			.unwrap()
			.status(),
		401,
		"another linked source must not substitute for the session's original source"
	);
	let sibling_access = sibling_tokens["access_token"].as_str().unwrap();
	assert_eq!(
		introspect(&client, &issuer, sibling_access).await["active"],
		true,
		"source disablement does not suspend the common Account or its other source"
	);

	// A completed common Account stop blocks both products; restoration does not
	// revive previously issued sessions or substitute another source identity.
	Accounts::new(auth.store.clone())
		.suspend(Administrator::Authentication, account.id, Utc::now())
		.await
		.unwrap();
	assert_eq!(
		introspect(&client, &issuer, sibling_access).await["active"],
		false,
		"a completed common Account stop reaches the independently active source too"
	);
	assert_eq!(
		client
			.get(format!("{marketplace_origin}/api/marketplace/library"))
			.header("cookie", &product_cookie)
			.send()
			.await
			.unwrap()
			.status(),
		401
	);
	Accounts::new(auth.store.clone())
		.restore(Administrator::Authentication, account.id)
		.await
		.unwrap();
	assert_eq!(
		client
			.get(format!("{marketplace_origin}/api/marketplace/library"))
			.header("cookie", &product_cookie)
			.send()
			.await
			.unwrap()
			.status(),
		401
	);

	// Desktop has an independent durable authentication session with its own
	// accepted lifetime; browser logout/expiry cannot silently replace it.
	let cloud_browser = auth
		.issue_session(sibling.id, "cloud", Utc::now())
		.await
		.unwrap();
	let desktop = auth
		.create_desktop_session(
			cloud_browser.id,
			"cloud",
			Uuid::new_v4(),
			90 * 24 * 3600,
			30 * 24 * 3600,
		)
		.await
		.unwrap();
	assert_eq!(desktop.account_id, account.id);
	assert_eq!(desktop.identity_id, sibling.id);
	assert_eq!(desktop.idle_seconds, 30 * 24 * 3600);
	assert!(desktop.absolute_expires_at > Utc::now() + chrono::Duration::days(89));
	assert!(
		auth.create_desktop_session(
			desktop.id,
			"cloud",
			Uuid::new_v4(),
			90 * 24 * 3600,
			30 * 24 * 3600
		)
		.await
		.is_err(),
		"a Desktop session cannot mint its own successor session hierarchy"
	);
	let mut expired_browser = cloud_browser.clone();
	expired_browser.absolute_expires_at = Utc::now() - chrono::Duration::seconds(1);
	let mut tx = auth.store.scope().await.unwrap();
	tx.save_session(&expired_browser).await.unwrap();
	Box::new(tx).commit().await.unwrap();
	assert_eq!(
		auth.check_session(desktop.id).await.unwrap().0,
		Validity::Active,
		"browser expiry must not shorten the Desktop contract"
	);
	Accounts::new(auth.store.clone())
		.logout(cloud_browser.id, Utc::now())
		.await
		.unwrap();
	assert_eq!(
		auth.check_session(desktop.id).await.unwrap().0,
		Validity::Active,
		"ordinary browser logout stays product/device-local"
	);
	let current = auth
		.issue_session(sibling.id, "marketplace", Utc::now())
		.await
		.unwrap();
	let admitted_sso = auth
		.issue_session(sibling.id, "sso", Utc::now())
		.await
		.unwrap();
	let continued = auth
		.continue_sso_session(admitted_sso.id, "marketplace")
		.await
		.unwrap();
	assert_eq!(continued.identity_id, sibling.id);
	assert_eq!(continued.issued_at, admitted_sso.issued_at);
	assert!(
		auth.continue_sso_session(current.id, "marketplace")
			.await
			.is_err(),
		"only an SSO browser session can be continued into a product session"
	);
	Accounts::new(auth.store.clone())
		.logout_all_authenticated(current.id, Utc::now())
		.await
		.unwrap();
	assert_ne!(
		auth.check_session(desktop.id).await.unwrap().0,
		Validity::Active,
		"explicit all-products logout includes independent Desktop sessions"
	);
	// The SSO check happened before this logout completed; continuing it now
	// must recheck the parent instead of minting a session at the new epoch.
	assert!(
		auth.continue_sso_session(admitted_sso.id, "marketplace")
			.await
			.is_err(),
		"an all-products logout that wins the race leaves no successor session"
	);

	let status: Value = client
		.post(format!("{issuer}/internal/accounts/check"))
		.basic_auth("marketplace-resource", Some(RESOURCE_SECRET))
		.json(&json!({"account_id":account.id}))
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(status["active"], true);
}

mod grpc_contract;
mod upstream_flow;

/// The shared physical database must not create a shared trust or FK boundary.
#[tokio::test]
#[allow(clippy::disallowed_methods)] // This test inspects isolated service credentials, never a global pool.
async fn service_database_roles_and_foreign_keys_preserve_ownership() {
	use reinhardt::db::orm::execution::convert_values;
	use reinhardt::query::{
		Alias, Expr, ExprTrait, IntoIden, JoinType, PostgresQueryBuilder, Query,
		QueryStatementBuilder, SimpleExpr,
	};
	let fixture = fixture(|_| {}).await;
	let marketplace = reinhardt::db::backends::DatabaseConnection::connect_postgres(
		&fixture.marketplace_database_url,
	)
	.await
	.unwrap();
	for (connection, role, foreign, ledger) in [
		(
			&fixture.auth.store.connection,
			"aidash_auth_app",
			"marketplace.marketplace_documents",
			"aidash_auth.reinhardt_migrations",
		),
		(
			&marketplace,
			"marketplace_app",
			"aidash_auth.auth_accounts",
			"marketplace.reinhardt_migrations",
		),
	] {
		// PostgreSQL reports true when ANY of these privileges are granted.
		for (table, privileges) in [
			(
				foreign,
				"SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER",
			),
			(ledger, "INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER"),
		] {
			let (sql, values) = Query::select()
				.expr_as(
					SimpleExpr::FunctionCall(
						"has_table_privilege".into_iden(),
						vec![
							Expr::value(role).into(),
							Expr::value(table).into(),
							Expr::value(privileges).into(),
						],
					),
					Alias::new("granted"),
				)
				.build(PostgresQueryBuilder);
			let row = fixture
				.admin
				.fetch_one(&sql, convert_values(values))
				.await
				.unwrap();
			assert_eq!(
				row.data.get("granted"),
				Some(&reinhardt::db::backends::QueryValue::Bool(false)),
				"unexpected privileges on {table}"
			);
		}
		let (schema, table) = foreign.split_once('.').unwrap();
		let (sql, values) = Query::select()
			.expr(Expr::value(1))
			.from((Alias::new(schema), Alias::new(table)))
			.build(PostgresQueryBuilder);
		assert!(
			connection
				.fetch_all(&sql, convert_values(values))
				.await
				.is_err(),
			"cross-service read must fail"
		);
		let (sql, values) = Query::delete()
			.from_table((Alias::new(schema), Alias::new(table)))
			.and_where(Expr::value(false).eq(true))
			.build(PostgresQueryBuilder);
		assert!(
			connection
				.execute(&sql, convert_values(values))
				.await
				.is_err(),
			"cross-service write must fail"
		);
	}
	let (sql, values) = Query::select()
		.column((Alias::new("pg_constraint"), Alias::new("conname")))
		.from(Alias::new("pg_constraint"))
		.join(
			JoinType::InnerJoin,
			Alias::new("pg_class"),
			Expr::col((Alias::new("pg_constraint"), Alias::new("confrelid")))
				.eq(Expr::col((Alias::new("pg_class"), Alias::new("oid")))),
		)
		.and_where(Expr::col((Alias::new("pg_constraint"), Alias::new("contype"))).eq("f"))
		.and_where(
			Expr::col((Alias::new("pg_constraint"), Alias::new("connamespace"))).ne(Expr::col((
				Alias::new("pg_class"),
				Alias::new("relnamespace"),
			))),
		)
		.build(PostgresQueryBuilder);
	assert!(
		fixture
			.admin
			.fetch_all(&sql, convert_values(values))
			.await
			.unwrap()
			.is_empty(),
		"migrations must not create cross-schema foreign keys"
	);
}

#[cfg(feature = "browser-e2e")]
mod browser_e2e;
