//! Actual HTTP and PostgreSQL tests own distribution authorization and durability.
//! Auth status is an external HTTP fixture; Marketplace data and decisions use
//! the real ORM, migrations, service handler, HTTP server and runtime role.
use crate::auth::{Session, hash};
use crate::{Config, Marketplace, migrations, store};
use aidash_domain::{
	agent_plugin::{PluginFile, tree_digest},
	marketplace::distribution::*,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::Utc;
use http::StatusCode;
use reinhardt::http::Handler;
use reinhardt::http::{Request, Response};
use serde_json::{Value, json as value};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

struct Status {
	account: Uuid,
	identity: Uuid,
	active: Arc<AtomicBool>,
}
#[async_trait::async_trait]
impl Handler for Status {
	async fn handle(&self, r: Request) -> reinhardt::core::exception::Result<Response> {
		let expected =
			STANDARD.encode("marketplace-resource:test-resource-secret-32-characters-long");
		if r.headers.get("authorization").and_then(|v| v.to_str().ok())
			!= Some(format!("Basic {expected}").as_str())
		{
			return Ok(Response::new(StatusCode::UNAUTHORIZED));
		}
		let body = if r.uri.path() == "/internal/accounts/check" {
			value!({"active":self.active.load(Ordering::SeqCst)})
		} else {
			value!({"active":self.active.load(Ordering::SeqCst),"sub":self.account,"account_id":self.account,"source_identity_id":self.identity,"client_id":"marketplace"})
		};
		Ok(Response::from_json_body(
			StatusCode::OK,
			serde_json::to_vec(&body).unwrap(),
		))
	}
}
async fn server(handler: impl Handler + 'static) -> (String, tokio::task::JoinHandle<()>) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	server_on(listener, handler).await
}
async fn server_on(
	listener: tokio::net::TcpListener,
	handler: impl Handler + 'static,
) -> (String, tokio::task::JoinHandle<()>) {
	let address = listener.local_addr().unwrap();
	let task = tokio::spawn(async move {
		reinhardt::server::HttpServer::new(handler)
			.with_max_body_size(12 * 1024 * 1024)
			.listen_on_with_shutdown(
				listener,
				reinhardt::server::ShutdownCoordinator::new(std::time::Duration::from_secs(2)),
			)
			.await
			.unwrap()
	});
	let client = reqwest::Client::new();
	let origin = format!("http://{address}");
	for _ in 0..100 {
		if client.get(format!("{origin}/healthz")).send().await.is_ok() {
			return (origin, task);
		}
		tokio::time::sleep(std::time::Duration::from_millis(20)).await;
	}
	panic!("HTTP server did not bind")
}
fn config(origin: &str, issuer: &str) -> Config {
	Config {
		assets_dir: if cfg!(feature = "browser-e2e") {
			Some(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dist"))
		} else {
			None
		},
		origin: origin.into(),
		issuer: issuer.into(),
		trusted_proxy_ips: vec![],
		client_id: "marketplace".into(),
		client_secret: "test-client-secret-32-characters-long".into(),
		billing_secret: "test-billing-secret-32-characters-long".into(),
		enrollment_secret: "test-enrollment-secret-32-characters-long".into(),
		recall_secret: "test-recall-secret-32-characters-long".into(),
		resource_client_id: "marketplace-resource".into(),
		resource_secret: "test-resource-secret-32-characters-long".into(),
	}
}

/// A migrated PostgreSQL database with the dedicated owner/runtime roles.
struct ProvisionedDatabase {
	admin: reinhardt::db::backends::DatabaseConnection,
	owner_url: String,
	app_url: String,
	_container: Box<dyn std::any::Any>,
}
// Only the fixture administrator provisions this service's schema and roles;
// application operations use the Marketplace runtime credential.
#[allow(clippy::disallowed_methods)]
async fn provisioned_database() -> ProvisionedDatabase {
	let (container, pool, _port, admin_url) = reinhardt::test::fixtures::postgres_container_with(
		reinhardt::test::fixtures::PostgresContainerConfig::default()
			.image("postgres", "17-alpine")
			.user("postgres")
			.password("fixture-admin-password")
			.database("marketplace"),
	)
	.await;
	let admin = reinhardt::db::backends::DatabaseConnection::connect_postgres(&admin_url)
		.await
		.unwrap();
	// Infrastructure DDL has no Query representation; this fixture provisions
	// exactly the dedicated owner/runtime roles required by production startup.
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
	for sql in [
		"ALTER ROLE marketplace_owner LOGIN PASSWORD 'fixture-owner-password'",
		"ALTER ROLE marketplace_app LOGIN PASSWORD 'fixture-runtime-password'",
		"REVOKE ALL ON SCHEMA public FROM PUBLIC",
	] {
		admin.execute(sql, Vec::new()).await.unwrap();
	}

	let with_login = |user: &str, password: &str| {
		let mut url = url::Url::parse(&admin_url).unwrap();
		url.set_username(user).unwrap();
		url.set_password(Some(password)).unwrap();
		url.to_string()
	};
	let owner_url = with_login("marketplace_owner", "fixture-owner-password");
	let app_url = with_login("marketplace_app", "fixture-runtime-password");

	migrations::migrate(&owner_url).await.unwrap();
	ProvisionedDatabase {
		admin,
		owner_url,
		app_url,
		_container: Box::new((container, pool)),
	}
}

#[tokio::test]
async fn standalone_distribution_enforces_ownership_and_persists_recall() {
	let ProvisionedDatabase {
		admin,
		owner_url,
		app_url,
		_container,
	} = provisioned_database().await;
	let account = Uuid::new_v4();
	let identity = Uuid::new_v4();
	let active = Arc::new(AtomicBool::new(true));
	let (issuer, auth_task) = server(Status {
		account,
		identity,
		active: active.clone(),
	})
	.await;
	let marketplace_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let origin = format!("http://{}", marketplace_listener.local_addr().unwrap());
	let origin = origin.as_str();
	assert!(
		Marketplace::connect(config(origin, &issuer), &owner_url)
			.await
			.is_err()
	);
	// A runtime credential that can forge migration history must never serve.
	admin
		.execute(
			"GRANT INSERT ON marketplace.reinhardt_migrations TO marketplace_app",
			Vec::new(),
		)
		.await
		.unwrap();
	assert!(
		Marketplace::connect(config(origin, &issuer), &app_url)
			.await
			.is_err()
	);
	admin
		.execute(
			"REVOKE INSERT ON marketplace.reinhardt_migrations FROM marketplace_app",
			Vec::new(),
		)
		.await
		.unwrap();
	let service = Marketplace::connect(config(origin, &issuer), &app_url)
		.await
		.unwrap();
	let token = crate::auth::secret();
	let csrf = crate::auth::secret();
	let session = Session {
		account_id: account,
		source_identity_id: identity,
		provider_subject: account.to_string(),
		csrf_token: csrf.clone(),
		refresh_request_id: Uuid::new_v4(),
		access_token: "fixture-access".into(),
		refresh_token: None,
		created_at: Utc::now(),
		last_used: Utc::now(),
	};
	service
		.store
		.connection
		.atomic(async |tx| {
			store::put(
				tx,
				&format!("session:{}", hash(&token)),
				"session",
				&session,
			)
			.await
		})
		.await
		.unwrap();
	let (endpoint, task) = server_on(
		marketplace_listener,
		crate::config::urls::handler(service.clone()),
	)
	.await;
	let client = reqwest::Client::new();
	let post = |path: &str, body: Value| {
		client
			.post(format!("{endpoint}{path}"))
			.header("cookie", format!("marketplace-session={token}"))
			.header("origin", origin)
			.header("x-marketplace-csrf", &csrf)
			.json(&body)
	};
	// The public Pages transport retains the same Product authority, never Auth's
	// tokens or privileges. These requests traverse the real server_fn router/DI.
	let rpc = format!("{endpoint}/api/server_fn");
	let response = client
		.post(format!("{rpc}/marketplace_publishers"))
		.json(&value!({}))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 401);
	let rejected: Value = response.json().await.unwrap();
	assert_eq!(rejected["version"], 1);
	assert_eq!(rejected["status"], 401);
	let response = client
		.post(format!("{rpc}/marketplace_session"))
		.header("cookie", format!("marketplace-session={token}"))
		.json(&value!({}))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 200);
	assert_eq!(response.headers()["cache-control"], "no-store");
	// An HTTP origin is loopback development, where HSTS would be meaningless.
	assert!(!response.headers().contains_key("strict-transport-security"));
	let current: Value = response.json().await.unwrap();
	assert_eq!(current, value!({"authenticated":true,"csrf_token":csrf}));
	let rejected = post(
		"/api/server_fn/marketplace_create_publisher",
		value!({"name":"Wrong proof","csrf":"wrong"}),
	)
	.send()
	.await
	.unwrap();
	assert_eq!(rejected.status(), 403);
	let wrong_origin = client
		.post(format!("{rpc}/marketplace_create_publisher"))
		.header("cookie", format!("marketplace-session={token}"))
		.header("origin", "https://other.example")
		.json(&value!({"name":"Wrong origin","csrf":csrf}))
		.send()
		.await
		.unwrap();
	assert_eq!(wrong_origin.status(), 403);
	for csrf_header in [Some("wrong"), None] {
		let mut request = client
			.post(format!("{endpoint}/api/marketplace/plugins"))
			.header("cookie", format!("marketplace-session={token}"))
			.header("origin", origin);
		if let Some(value) = csrf_header {
			request = request.header("x-marketplace-csrf", value);
		}
		let rejected = request.json(&value!({})).send().await.unwrap();
		assert_eq!(rejected.status(), 403, "{csrf_header:?}");
	}

	// An HTTPS origin makes the application refuse plain HTTP itself and send
	// HSTS; only a configured proxy IP may assert forwarded HTTPS.
	let hsts = "max-age=63072000; includeSubDomains";
	for (proxies, forwarded, path, status) in [
		(&["127.0.0.1"][..], false, "/healthz", 200),
		(&["127.0.0.1"], false, "/api/marketplace/catalog", 400),
		(
			&["127.0.0.1"],
			false,
			"/api/server_fn/marketplace_session",
			400,
		),
		(&["127.0.0.1"], true, "/api/marketplace/catalog", 200),
		(&[], true, "/api/marketplace/catalog", 400),
		// A CIDR network trusts any peer inside it and no other.
		(&["127.0.0.0/8"], true, "/api/marketplace/catalog", 200),
		(&["10.0.0.0/8"], true, "/api/marketplace/catalog", 400),
		(
			&["10.0.0.0/8", "127.0.0.0/8"],
			true,
			"/api/marketplace/catalog",
			200,
		),
	] {
		let secure = Marketplace::connect(
			Config {
				trusted_proxy_ips: proxies.iter().map(|proxy| proxy.parse().unwrap()).collect(),
				..config("https://marketplace.example.com", &issuer)
			},
			&app_url,
		)
		.await
		.unwrap();
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let (secure_endpoint, secure_task) =
			server_on(listener, crate::config::urls::handler(secure)).await;
		let url = format!("{secure_endpoint}{path}");
		let mut request = if path.starts_with("/api/server_fn/") {
			client.post(url)
		} else {
			client.get(url)
		};
		if forwarded {
			request = request.header("x-forwarded-proto", "https");
		}
		let response = request.send().await.unwrap();
		assert_eq!(response.status(), status, "{path} {proxies:?} {forwarded}");
		assert_eq!(response.headers()["strict-transport-security"], hsts);
		secure_task.abort();
	}
	// Missing entitlement must reach the real persistence-backed acquisition guard.
	let publisher: Value = post(
		"/api/server_fn/marketplace_create_publisher",
		value!({"name":"HTTP Publisher","csrf":csrf}),
	)
	.send()
	.await
	.unwrap()
	.json()
	.await
	.unwrap();
	let publisher = publisher["publisher"]["id"].as_str().unwrap();
	let slug = format!("plugin-{}", Uuid::new_v4().simple());
	let created:Value=post("/api/marketplace/plugins",value!({"publisher_id":publisher,"slug":slug,"name":"Plugin","description":"Example","price_cents":0,"currency":"USD","declared_permissions":["read"],"license":"MIT","origin":"https://example.com/plugin"})).send().await.unwrap().json().await.unwrap();
	let plugin = created["plugin"]["id"].as_str().unwrap();
	let manifest = br#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"http-plugin"}"#;
	let files = [PluginFile {
		path: "plugin.json".into(),
		bytes: manifest.to_vec(),
	}];
	let published=post(&format!("/api/marketplace/plugins/{plugin}/versions"),value!({"version":"1.0.0","files":{"plugin.json":STANDARD.encode(manifest)},"expected_sha256":tree_digest(&files)})).send().await.unwrap();
	assert_eq!(
		published.status(),
		200,
		"{}",
		published.text().await.unwrap()
	);
	let public: Value = client
		.get(format!("{endpoint}/api/marketplace/catalog/{slug}"))
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(public["plugin"]["license"], "MIT");
	let download = format!("/api/marketplace/plugins/{plugin}/versions/1.0.0/download");
	assert_eq!(
		client
			.get(format!("{endpoint}{download}"))
			.header("cookie", format!("marketplace-session={token}"))
			.send()
			.await
			.unwrap()
			.status(),
		403
	);

	#[cfg(feature = "browser-e2e")]
	let browser = browser_e2e::acquire(&endpoint, &slug, plugin, &token).await;
	assert_eq!(
		post(
			&format!("/api/marketplace/plugins/{plugin}/acquire"),
			value!({})
		)
		.send()
		.await
		.unwrap()
		.status(),
		200
	);
	let artifact: Artifact = client
		.get(format!("{endpoint}{download}"))
		.header("cookie", format!("marketplace-session={token}"))
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(artifact.digest, tree_digest(&files));
	artifact.validate().unwrap();
	assert_eq!(
		post(
			&format!("/api/marketplace/plugins/{plugin}/versions"),
			value!({"version":"1.0.0","files":{"plugin.json":STANDARD.encode(manifest)}})
		)
		.send()
		.await
		.unwrap()
		.status(),
		409
	);

	// A paid browser claim cannot manufacture an Entitlement. Only the trusted
	// Billing service can grant one, scoped to the full Deployment/Tenant pair.
	let paid:Value=post("/api/marketplace/plugins",value!({"publisher_id":publisher,"slug":format!("paid-{}",Uuid::new_v4().simple()),"name":"Paid","description":"Paid fixture","price_cents":100,"currency":"USD","declared_permissions":[],"license":"Commercial","origin":"https://example.com/paid"})).send().await.unwrap().json().await.unwrap();
	let paid = paid["plugin"]["id"].as_str().unwrap();
	assert_eq!(
		post(
			&format!("/api/marketplace/plugins/{paid}/versions"),
			value!({"version":"1.0.0","files":{"plugin.json":STANDARD.encode(manifest)}})
		)
		.send()
		.await
		.unwrap()
		.status(),
		200
	);
	assert_eq!(
		post(
			&format!("/api/marketplace/plugins/{paid}/acquire"),
			value!({"purchased":true})
		)
		.send()
		.await
		.unwrap()
		.status(),
		403
	);
	let billing = value!({"event_id":format!("event-{}",Uuid::new_v4()),"plugin_id":paid,"owner":{"kind":"tenant","id":{"deployment_id":"deployment-a","tenant_id":"same-name"}},"expires_at":null,"revoked":false});
	assert_eq!(
		client
			.post(format!("{endpoint}/internal/billing/events"))
			.bearer_auth("untrusted-browser")
			.json(&billing)
			.send()
			.await
			.unwrap()
			.status(),
		403
	);
	let grant = || {
		client
			.post(format!("{endpoint}/internal/billing/events"))
			.bearer_auth("test-billing-secret-32-characters-long")
			.json(&billing)
	};
	assert_eq!(grant().send().await.unwrap().status(), 200);
	assert_eq!(
		grant().send().await.unwrap().status(),
		200,
		"an identical verified event retry is idempotent"
	);
	// The three /internal/ POST routes are machine-to-machine and keep their in-cluster
	// plain-HTTP path under an HTTPS origin with no trusted proxy; every other path or
	// method, browser and Registry routes stay refused.
	let strict = Marketplace::connect(config("https://marketplace.example.com", &issuer), &app_url)
		.await
		.unwrap();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let (strict_endpoint, strict_task) =
		server_on(listener, crate::config::urls::handler(strict)).await;
	let mut internal = billing.clone();
	internal["event_id"] = format!("event-{}", Uuid::new_v4()).into();
	assert_eq!(
		client
			.post(format!("{strict_endpoint}/internal/billing/events"))
			.bearer_auth("test-billing-secret-32-characters-long")
			.json(&internal)
			.send()
			.await
			.unwrap()
			.status(),
		200,
		"plain-HTTP /internal/* is not refused for transport"
	);
	assert_eq!(
		client
			.post(format!("{strict_endpoint}/internal/billing/events"))
			.bearer_auth("untrusted-browser")
			.json(&internal)
			.send()
			.await
			.unwrap()
			.status(),
		403,
		"exempt from HTTPS, not from the Bearer credential"
	);
	for (path, method_post) in [
		("/api/marketplace/catalog", false),
		("/api/marketplace/downloads", true),
		("/internal/other", true),
		("/internal/recalls", false),
	] {
		let url = format!("{strict_endpoint}{path}");
		let request = if method_post {
			client.post(url)
		} else {
			client.get(url)
		};
		assert_eq!(request.send().await.unwrap().status(), 400, "{path}");
	}
	strict_task.abort();
	let mut changed = billing.clone();
	changed["revoked"] = true.into();
	assert_eq!(
		client
			.post(format!("{endpoint}/internal/billing/events"))
			.bearer_auth("test-billing-secret-32-characters-long")
			.json(&changed)
			.send()
			.await
			.unwrap()
			.status(),
		409
	);
	let mut connections = Vec::new();
	for deployment in ["deployment-a", "deployment-b"] {
		let enrollment: Value = client
			.post(format!("{endpoint}/internal/enrollments"))
			.bearer_auth("test-enrollment-secret-32-characters-long")
			.json(&value!({"deployment_id":deployment,"tenant_id":"same-name"}))
			.send()
			.await
			.unwrap()
			.json()
			.await
			.unwrap();
		let connection: Value = post(
			"/api/marketplace/connections",
			value!({"deployment_id":deployment,"enrollment_token":enrollment["enrollment_token"]}),
		)
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
		connections.push(connection);
	}
	for (index, connection) in connections.iter().enumerate() {
		let request = value!({"connection_id":connection["connection"]["id"],"owner_account_id":account,"owner_tenant_id":"same-name","plugin_id":paid,"version":"1.0.0"});
		assert_eq!(
			client
				.post(format!("{endpoint}/api/marketplace/downloads"))
				.bearer_auth(connection["credential"].as_str().unwrap())
				.json(&request)
				.send()
				.await
				.unwrap()
				.status(),
			if index == 0 { 200 } else { 403 },
			"a same-named Tenant on another Deployment has no grant"
		);
	}
	let connection = &connections[0];
	assert_eq!(client.post(format!("{endpoint}/api/marketplace/downloads")).bearer_auth(connection["credential"].as_str().unwrap()).json(&value!({"connection_id":connection["connection"]["id"],"owner_account_id":Uuid::new_v4(),"owner_tenant_id":"same-name","plugin_id":paid,"version":"1.0.0"})).send().await.unwrap().status(),403);
	// A malformed tree is rejected at the publication boundary, not installed.
	assert_eq!(
		post(
			&format!("/api/marketplace/plugins/{plugin}/versions"),
			value!({"version":"2.0.0","files":{"../plugin.json":STANDARD.encode(manifest)}})
		)
		.send()
		.await
		.unwrap()
		.status(),
		400
	);

	// The immutable envelope may supply an optional manifest version, but an
	// explicitly declared manifest version must never disagree with it.
	let mismatched=br#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"http-plugin","version":"9.0.0"}"#;
	for (version, manifest) in [
		("2.0.0", mismatched.as_slice()),
		("latest", manifest.as_slice()),
	] {
		assert_eq!(
			post(
				&format!("/api/marketplace/plugins/{plugin}/versions"),
				value!({"version":version,"files":{"plugin.json":STANDARD.encode(manifest)}})
			)
			.send()
			.await
			.unwrap()
			.status(),
			400,
			"invalid or inconsistent immutable version must be rejected"
		);
	}

	// A valid near-8-MiB raw tree expands beyond the framework's old 10-MiB
	// default. Exercise actual HTTP collection, publication and persistence.
	let large = vec![b'x'; MAX_ARTIFACT_BYTES - manifest.len()];
	assert_eq!(post(&format!("/api/marketplace/plugins/{plugin}/versions"),value!({"version":"3.0.0","files":{"plugin.json":STANDARD.encode(manifest),"payload.bin":STANDARD.encode(&large)}})).send().await.unwrap().status(),200,"a valid 8-MiB decoded tree must pass the network body collector");
	let oversized = vec![b'x'; MAX_ARTIFACT_BYTES - manifest.len() + 1];
	assert_eq!(post(&format!("/api/marketplace/plugins/{plugin}/versions"),value!({"version":"3.0.1","files":{"plugin.json":STANDARD.encode(manifest),"payload.bin":STANDARD.encode(&oversized)}})).send().await.unwrap().status(),400,"the decoded limit remains authoritative after base64 transport");

	// Lifecycle changes refuse the next HTTP admission despite an unexpired cookie.
	active.store(false, Ordering::SeqCst);
	assert_eq!(
		client
			.get(format!("{endpoint}/api/marketplace/library"))
			.header("cookie", format!("marketplace-session={token}"))
			.send()
			.await
			.unwrap()
			.status(),
		401
	);
	assert_eq!(
		client
			.get(format!("{endpoint}/api/marketplace/catalog"))
			.send()
			.await
			.unwrap()
			.status(),
		200
	);
	active.store(true, Ordering::SeqCst);
	assert_eq!(
		post(
			&format!("/api/marketplace/plugins/{plugin}/versions/1.0.0/recall"),
			value!({"reason":"publisher cannot order an Aidash Recall"})
		)
		.send()
		.await
		.unwrap()
		.status(),
		404
	);
	assert_eq!(
		client
			.post(format!("{endpoint}/internal/recalls"))
			.bearer_auth("test-billing-secret-32-characters-long")
			.json(
				&value!({"plugin_id":plugin,"version":"1.0.0","reason":"wrong service permission"})
			)
			.send()
			.await
			.unwrap()
			.status(),
		403
	);

	assert_eq!(
		client
			.post(format!("{endpoint}/internal/recalls"))
			.bearer_auth("test-recall-secret-32-characters-long")
			.json(&value!({"plugin_id":plugin,"version":"1.0.0","reason":"unsafe version"}))
			.send()
			.await
			.unwrap()
			.status(),
		200
	);

	#[cfg(feature = "browser-e2e")]
	browser_e2e::recalled(&browser, &endpoint, plugin).await;
	// Reconnect through a fresh service proves this is not a process-memory list.
	let restarted = Marketplace::connect(config(origin, &issuer), &app_url)
		.await
		.unwrap();
	let recalls = restarted.store.all::<Recall>("recall").await.unwrap();
	assert!(
		recalls
			.iter()
			.any(|r| r.plugin_id.to_string() == plugin && r.reason == "unsafe version")
	);
	assert_eq!(
		client
			.get(format!("{endpoint}{download}"))
			.header("cookie", format!("marketplace-session={token}"))
			.send()
			.await
			.unwrap()
			.status(),
		403
	);

	#[cfg(feature = "browser-e2e")]
	{
		active.store(false, Ordering::SeqCst);
		browser_e2e::stopped(browser, &endpoint).await;
	}
	auth_task.abort();
	tokio::time::sleep(std::time::Duration::from_millis(20)).await;
	assert_eq!(
		client
			.get(format!("{endpoint}/api/marketplace/library"))
			.header("cookie", format!("marketplace-session={token}"))
			.send()
			.await
			.unwrap()
			.status(),
		503
	);
	assert_eq!(
		client
			.get(format!("{endpoint}/api/marketplace/catalog"))
			.send()
			.await
			.unwrap()
			.status(),
		200
	);
	let logout = post("/auth/logout", value!({})).send().await.unwrap();
	assert_eq!(
		logout.status(),
		200,
		"local logout works during dependency outage"
	);
	assert!(
		logout
			.headers()
			.get("set-cookie")
			.unwrap()
			.to_str()
			.unwrap()
			.contains("Max-Age=0")
	);
	task.abort();
}

#[cfg(feature = "browser-e2e")]
mod browser_e2e;
