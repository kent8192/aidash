use aidash_server::{
	config::Config, domain::qualified_agent, federation::Federation, registry::Registry,
	store::Store,
};
use reinhardt::db::backends::{DatabaseConnection, dialect::PostgresBackend};
use reinhardt::db::migrations::{FilesystemSource, MigrationSource};
use reinhardt::test::fixtures::http_client;
use serde_json::{Value, json};
use sqlx::{
	Connection, Executor,
	postgres::{PgConnectOptions, PgConnection, PgPoolOptions},
};
use std::{future::Future, pin::Pin};
use std::{path::PathBuf, str::FromStr, sync::Arc};
use uuid::Uuid;

#[path = "environment.rs"]
mod environment;
#[allow(unused_imports)] // Sixteen-Node acceptance cases need a separate connection budget.
pub use environment::isolated_test_environment;
use environment::postgres;
pub use environment::{TestEnvironment, test_environment};
#[path = "application.rs"]
mod application;
#[allow(unused_imports)] // Each binary uses only its required fixture constructors.
pub use application::{
	ApplicationFixture, ApplicationFuture, RouterTransform, TestApplication, application,
	application_with, application_with_event_streams, application_with_settings,
	direct_application, native_application, peer_application,
};
#[allow(unused_imports)] // Worker-process fixtures share this settings module.
pub(crate) use application::{process_settings, settings_for};

#[allow(dead_code)] // Header-sensitive browser and peer tests share this request dispatcher.
pub async fn http_response(
	app: &TestApplication,
	method: &str,
	path: &str,
	headers: &[(&str, &str)],
	body: &[u8],
) -> reinhardt::test::TestResponse {
	if method == "GET" {
		return app.client().get_with_headers(path, headers).await.unwrap();
	}
	if method == "POST"
		&& let Some((_, content_type)) = headers
			.iter()
			.find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
	{
		return app
			.client()
			.post_raw_with_headers(path, body, content_type, headers)
			.await
			.unwrap();
	}
	// reinhardt-web#6661: other verbs and an intentionally absent Content-Type
	// need the declared raw client because the native generic dispatcher is private.
	let mut request = app
		.raw_http
		.request(method.parse().unwrap(), app.url(path))
		.body(body.to_owned());
	for (name, value) in headers {
		request = request.header(*name, *value);
	}
	let response = request.send().await.unwrap();
	let status = response.status();
	let headers = response.headers().clone();
	let version = response.version();
	let body = response.bytes().await.unwrap();
	reinhardt::test::TestResponse::with_body_and_version(status, headers, body, version)
}

#[allow(dead_code)] // Shared fixtures are used by different integration-test binaries.
pub async fn request(
	app: &TestApplication,
	token: &str,
	method: &str,
	path: &str,
	value: Value,
) -> (u16, Value) {
	let authorization = format!("Bearer {token}");
	let headers = [("Authorization", authorization.as_str())];
	let response = match method {
		"GET" => Some(app.api_http.get_with_headers(path, &headers).await.unwrap()),
		"POST" => Some(
			app.api_http
				.post_raw_with_headers(
					path,
					value.to_string().as_bytes(),
					"application/json",
					&headers,
				)
				.await
				.unwrap(),
		),
		_ => None,
	};
	if let Some(response) = response {
		return json_response(method, path, response.status_code(), response.body());
	}
	// The pinned APIClient has per-request headers only for GET and raw POST.
	// reinhardt-web#6661: a fixture-owned raw client keeps concurrent
	// PUT/PATCH/DELETE credentials isolated until every verb supports headers.
	let mut request = app
		.raw_http
		.request(method.parse().unwrap(), app.url(path))
		.bearer_auth(token);
	if matches!(method, "PUT" | "PATCH") || (method == "DELETE" && !value.is_null()) {
		request = request.json(&value);
	}
	let response = request.send().await.unwrap();
	let status = response.status().as_u16();
	let body = response.bytes().await.unwrap();
	json_response(method, path, status, &body)
}

fn json_response(method: &str, path: &str, status: u16, body: &[u8]) -> (u16, Value) {
	if method == "HEAD" || matches!(status, 204 | 205 | 304) {
		assert!(body.is_empty(), "bodyless response for {method} {path}");
		(status, Value::Null)
	} else {
		(
			status,
			serde_json::from_slice(body).expect("JSON endpoint response"),
		)
	}
}

#[allow(dead_code)] // Shared fixtures are used by different integration-test binaries.
pub fn setup(
	environment: &TestEnvironment,
) -> Pin<Box<dyn Future<Output = (Federation, String, String)> + Send + '_>> {
	Box::pin(async move {
		let database = format!("execution_{}", Uuid::new_v4().simple());
		// The preserved baseline names public explicitly. Isolate databases, not
		// search paths. TestEnvironment owns the container and every database, so
		// unwinding also removes fixtures that never reach explicit cleanup.
		let mut admin = PgConnection::connect(&environment.database_url)
			.await
			.unwrap();
		admin
			.execute(format!("CREATE DATABASE {database}").as_str())
			.await
			.unwrap();
		let mut url = reqwest::Url::parse(&environment.database_url).unwrap();
		url.set_path(&database);
		let url = url.to_string();
		let options = PgConnectOptions::from_str(&url)
			.unwrap()
			.application_name(&database);
		let pool = PgPoolOptions::new()
			.max_connections(12)
			.connect_with(options)
			.await
			.unwrap();
		let connection = DatabaseConnection::new(Arc::new(PostgresBackend::new(pool.clone())));
		let migrations =
			FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
				.all_migrations()
				.await
				.expect("load the native migration graph");
		postgres::apply_migrations(connection, &migrations).await;
		let store = Store::from_pool(pool.clone(), "aidash://execution-test".into())
			.await
			.unwrap();
		let federation = Federation {
			sandbox: Default::default(),
			store,
			registry: Registry::new(pool, "aidash://execution-test").unwrap(),
			config: Config {
				node_id: "aidash://execution-test".into(),
				endpoint: "http://localhost:8080".into(),
				database_url: url.clone(),
				nats_url: environment.nats_url.clone(),
				api_token: "operator-execution-fixture".into(),
				web_dir: "web/dist".into(),
				lease_seconds: 30,
				default_host_packages: vec![],
				oidc: None,
			},
			client: reqwest::Client::new(),
			notify: Arc::new(tokio::sync::Notify::new()),
		};
		(federation, url, database)
	})
}

#[allow(dead_code)] // Paired with setup in the integration-test binaries that use it.
pub async fn cleanup(f: Federation, url: &str, database: &str) {
	f.store.control_pool.close().await;
	f.store.pool.close().await;
	cleanup_database(url, database).await;
}

#[allow(dead_code)] // The persistence-only suite shares the same database lifecycle.
pub async fn cleanup_database(_url: &str, database: &str) {
	assert!(database.starts_with("execution_"));
	assert!(
		database
			.bytes()
			.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
	);
	// TestEnvironment owns the entire disposable postmaster. Retain isolated
	// databases until its container is dropped: DROP DATABASE broadcasts a
	// process barrier even to other databases and can block behind concurrent
	// PGroonga index initialization. Callers close their process-owned pools;
	// container teardown removes every fixture database together.
}

#[allow(dead_code)]
pub fn policy(node: &str) -> Value {
	let agent = qualified_agent(node, "research", "1.0.0");
	json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},agent:{"kind":"agent"}},
        "policies":[{"id":"approved-work","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]})
}

#[allow(dead_code)]
pub async fn bootstrap(
	f: &Federation,
	app: &TestApplication,
	endpoint: &str,
) -> (Value, String, Uuid) {
	let operator = &f.config.api_token;
	let policy = policy(&f.config.node_id);
	let (status, snapshot) = request(
		app,
		operator,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":0,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "initial policy: {snapshot}");
	for (kind, id, config) in [
		(
			"model",
			"model",
			json!({"provider":"openrouter","model_id":"fixture","endpoint":format!("{endpoint}/v1"),"context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}),
		),
		(
			"tool",
			"http",
			json!({"transport":"http","endpoint":format!("{endpoint}/effect"),"credential_env":null,"replay":"idempotent"}),
		),
		(
			"agent",
			"research",
			json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Test approved work","tools":[{"id":"http","version":"1.0.0"}],"skills":[]}),
		),
	] {
		let entry = json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"fixture"},"capabilities":[],"languages":["en"],"schema":{"type":"object"},"config":config});
		assert_eq!(
			request(app, operator, "POST", "/api/registry", entry)
				.await
				.0,
			200
		);
		let (status, response) = request(
			app,
			operator,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":id,"version":"1.0.0"},"expected_revision":0,"enabled":true}),
		)
		.await;
		assert_eq!(status, 200, "catalog admission: {response}");
	}
	let (status, credential) = request(
		app,
		operator,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	assert_eq!(status, 200);
	let token = credential["token"].as_str().unwrap().to_owned();
	let (status, workspace) = request(
		app,
		&token,
		"POST",
		"/api/workspaces",
		json!({"title":"Approved task","goal":"Use exactly one tool"}),
	)
	.await;
	assert_eq!(status, 200);
	let (status, task) = request(
		app,
		&token,
		"POST",
		&format!(
			"/api/workspaces/{}/tasks",
			workspace["id"].as_str().unwrap()
		),
		json!({"title":"Research","description":"Use approved tools"}),
	)
	.await;
	assert_eq!(status, 200);
	(
		policy,
		token,
		Uuid::parse_str(task["id"].as_str().unwrap()).unwrap(),
	)
}

/// Explicit native process settings for the fixture's isolated database.
#[allow(dead_code)]
pub fn native_process_environment(
	f: &Federation,
	database_url: &str,
	directory: &std::path::Path,
	worker_count: usize,
) -> Vec<(&'static str, String)> {
	let mut settings = settings_for(database_url);
	let database = reqwest::Url::parse(database_url).unwrap();
	settings
		.core
		.databases
		.get_mut("default")
		.unwrap()
		.options
		.extend(
			database
				.query_pairs()
				.map(|(key, value)| (key.to_string(), value.to_string())),
		);
	settings.node.node_id = f.config.node_id.clone();
	settings.node.endpoint = f.config.endpoint.clone();
	settings.node.api_token = f.config.api_token.clone();
	settings.node.nats_url = f.config.nats_url.clone();
	settings.node.web_dir = f.config.web_dir.clone();
	settings.node.background_enabled = true;
	settings.node.worker_count = worker_count;
	settings.dashboard.oidc = f.config.oidc.clone();
	let destination = directory.join(format!("settings-{}", Uuid::new_v4()));
	std::fs::create_dir_all(&destination).unwrap();
	std::fs::write(destination.join("base.toml"), process_settings(&settings)).unwrap();
	let listen = reqwest::Url::parse(&f.config.endpoint)
		.unwrap()
		.socket_addrs(|| None)
		.unwrap();
	assert_eq!(listen.len(), 1, "fixture must advertise one local listener");
	vec![
		// Multi-process fixtures need bounded executors even on high-core hosts.
		("TOKIO_WORKER_THREADS", "2".into()),
		(
			"REINHARDT_SETTINGS_DIR",
			destination.to_string_lossy().into_owned(),
		),
		("REINHARDT_ENV", "container".into()),
		("AIDASH_LISTEN", listen[0].to_string()),
	]
}
#[allow(dead_code)]
pub fn native_process_args(_f: &Federation, mode: &str) -> Vec<String> {
	vec![mode.to_owned()]
}

#[allow(dead_code)]
pub async fn native_store(database_url: &str, node: &str) -> Store {
	let pool = sqlx::PgPool::connect(database_url).await.unwrap();
	let connection = DatabaseConnection::new(Arc::new(PostgresBackend::new(pool.clone())));
	let migrations =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
			.all_migrations()
			.await
			.unwrap();
	postgres::apply_migrations(connection, &migrations).await;
	Store::from_pool(pool, node.to_owned()).await.unwrap()
}

#[path = "state.rs"]
mod state;
#[allow(unused_imports)] // Shared fixture exports vary by integration target.
pub use state::{context, pending, tool_call, tool_pending};

use environment::EnvironmentFuture;
use futures_util::{
	FutureExt,
	future::{BoxFuture, Shared},
};
use rstest::fixture;

pub type DatabaseFuture = Shared<BoxFuture<'static, Arc<ExecutionDatabase>>>;
pub type RuntimeFuture = Shared<BoxFuture<'static, RuntimeFixture>>;

pub struct ExecutionDatabase {
	pub pool: sqlx::PgPool,
	pub url: String,
	pub schema: String,
	environment: Arc<TestEnvironment>,
}

#[derive(Clone)]
#[allow(dead_code)] // Shared integration binaries independently use each runtime value.
pub struct RuntimeFixture {
	pub federation: Federation,
	pub url: String,
	pub schema: String,
	_database: Arc<ExecutionDatabase>,
}

impl RuntimeFixture {
	#[allow(dead_code)] // Process and protocol suites inspect the retained broker environment.
	pub fn environment(&self) -> Arc<TestEnvironment> {
		self._database.environment.clone()
	}

	/// Project values while this fixture retains its disposable infrastructure.
	#[allow(dead_code)] // Database-only and process-only targets do not unpack HTTP runtime state.
	pub fn parts(&self) -> (Federation, String, String) {
		(
			self.federation.clone(),
			self.url.clone(),
			self.schema.clone(),
		)
	}
}

#[fixture]
pub fn execution_database(test_environment: EnvironmentFuture) -> DatabaseFuture {
	async move {
		let environment = test_environment.await;

		let database = format!("execution_{}", Uuid::new_v4().simple());
		// The preserved baseline names public explicitly. Isolate databases, not
		// search paths. TestEnvironment owns the container and every database, so
		// unwinding also removes fixtures that never reach explicit cleanup.
		let mut admin = PgConnection::connect(&environment.database_url)
			.await
			.unwrap();
		admin
			.execute(format!("CREATE DATABASE {database}").as_str())
			.await
			.unwrap();
		let mut url = reqwest::Url::parse(&environment.database_url).unwrap();
		url.set_path(&database);
		let url = url.to_string();
		let options = PgConnectOptions::from_str(&url)
			.unwrap()
			.application_name(&database);
		let pool = PgPoolOptions::new()
			.max_connections(12)
			.connect_with(options)
			.await
			.unwrap();
		let connection = DatabaseConnection::new(Arc::new(PostgresBackend::new(pool.clone())));
		let migrations =
			FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
				.all_migrations()
				.await
				.expect("load the native migration graph");
		postgres::apply_migrations(connection, &migrations).await;
		Arc::new(ExecutionDatabase {
			pool,
			url,
			schema: database,
			environment,
		})
	}
	.boxed()
	.shared()
}

#[fixture]
pub fn runtime(execution_database: DatabaseFuture, http_client: reqwest::Client) -> RuntimeFuture {
	async move {
		let database = execution_database.await;

		let store = Store::from_pool(database.pool.clone(), "aidash://execution-test".into())
			.await
			.unwrap();
		let federation = Federation {
			sandbox: Default::default(),
			store,
			registry: Registry::new(database.pool.clone(), "aidash://execution-test").unwrap(),
			config: Config {
				node_id: "aidash://execution-test".into(),
				endpoint: "http://localhost:8080".into(),
				database_url: database.url.clone(),
				nats_url: database.environment.nats_url.clone(),
				api_token: "operator-execution-fixture".into(),
				web_dir: "web/dist".into(),
				lease_seconds: 30,
				default_host_packages: vec![],
				oidc: None,
			},
			client: http_client,
			notify: Arc::new(tokio::sync::Notify::new()),
		};
		RuntimeFixture {
			federation,
			url: database.url.clone(),
			schema: database.schema.clone(),
			_database: database,
		}
	}
	.boxed()
	.shared()
}
