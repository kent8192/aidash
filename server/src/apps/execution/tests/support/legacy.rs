use aidash_server::{
	authorization::Authorization,
	config::Config,
	domain::qualified_agent,
	federation::Federation,
	registry::{EntityRef, Registry},
	store::Store,
};
use reinhardt::db::backends::{DatabaseConnection, dialect::PostgresBackend};
use reinhardt::db::migrations::{FilesystemSource, MigrationSource};
use reinhardt::test::fixtures::http_client;
use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{path::PathBuf, str::FromStr, sync::Arc};
use uuid::Uuid;

#[allow(dead_code)] // Browser-authentication suites seed current-format sessions directly.
pub fn browser_session_digest(value: impl AsRef<[u8]>) -> Vec<u8> {
	use sha2::Digest as _;

	let mut digest = sha2::Sha256::new();
	digest.update(b"aidash-browser-session-v2\0");
	digest.update(value.as_ref());
	digest.finalize().to_vec()
}

#[path = "environment.rs"]
mod environment;
#[allow(unused_imports)] // Only suites whose tests have hung use the whole-test backstop.
pub use environment::TEST_BOUND;
#[allow(unused_imports)] // Sixteen-Node acceptance cases need a separate connection budget.
pub use environment::isolated_test_environment;
use environment::postgres;
#[allow(unused_imports)] // Only fault-injection suites change the schema.
pub use environment::postgres::{admin_pool, owner_pool};
pub use environment::{TestEnvironment, test_environment};
#[path = "application.rs"]
mod application;
#[allow(unused_imports)] // Each binary uses only its required fixture constructors.
pub use application::{
	ApplicationFixture, ApplicationFuture, RouterTransform, TestApplication, application,
	application_with, application_with_event_streams, application_with_settings,
	direct_application, native_application, streaming_http_client,
};
#[path = "peer.rs"]
mod peer;
#[path = "upstream.rs"]
pub mod upstream_fixtures;
#[allow(unused_imports)] // Worker-process fixtures share this settings module.
pub(crate) use application::{process_settings, settings_for};
#[allow(unused_imports)] // Only federation suites consume fixed-origin application fixtures.
pub use peer::{FixedServerGuard as PeerServerGuard, PeerFixture, PeerFuture, native_peer};

#[path = "tenant_roots.rs"]
mod tenant_roots;
#[allow(unused_imports)] // Only suites that create Tenants under fixed names use these.
pub use tenant_roots::{
	admitted_run, approve_agent, local_run, remote_run, remote_run_in, replace_policy, tenant_root,
	tenant_root_in, tenant_scope, tenant_token,
};

#[allow(dead_code)] // Header-sensitive browser and peer tests share this request dispatcher.
pub async fn http_response(
	app: &TestApplication,
	method: &str,
	path: &str,
	headers: &[(&str, &str)],
	body: &[u8],
) -> reinhardt::test::TestResponse {
	let client = &(app.client());
	let mut request = client
		.request(method.parse().unwrap(), path)
		.body(bytes::Bytes::copy_from_slice(body));
	for (name, value) in headers {
		request = request.header(*name, *value);
	}
	request.send().await.unwrap()
}

#[allow(dead_code)] // Shared fixtures are used by different integration-test binaries.
pub async fn request(
	app: &TestApplication,
	token: &str,
	method: &str,
	path: &str,
	value: Value,
) -> (u16, Value) {
	let mut request = app
		.api_http
		.request(method.parse().unwrap(), path)
		.header("Authorization", format!("Bearer {token}"));
	if matches!(method, "POST" | "PUT" | "PATCH") || (method == "DELETE" && !value.is_null()) {
		request = request.json(&value);
	}
	let response = request.send().await.unwrap();
	json_response(method, path, response.status_code(), response.body())
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

#[allow(dead_code)] // Explicit teardown Acts close resources before lifecycle assertions.
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
	bootstrap_with_context(f, app, endpoint, false).await
}
#[allow(dead_code)]
pub async fn bootstrap_with_context(
	f: &Federation,
	app: &TestApplication,
	endpoint: &str,
	semantic: bool,
) -> (Value, String, Uuid) {
	bootstrap_with_resources(f, app, endpoint, semantic, false).await
}
#[allow(dead_code)]
pub async fn bootstrap_with_area(
	f: &Federation,
	app: &TestApplication,
	endpoint: &str,
) -> (Value, String, Uuid) {
	bootstrap_with_resources(f, app, endpoint, false, true).await
}
async fn bootstrap_with_resources(
	f: &Federation,
	app: &TestApplication,
	endpoint: &str,
	semantic: bool,
	files: bool,
) -> (Value, String, Uuid) {
	let deployment_administrator = &f.config.api_token;
	let policy = policy(&f.config.node_id);
	tenant_root(f, "acme").await;
	let (status, snapshot) = request(
		app,
		deployment_administrator,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":0,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "initial policy: {snapshot}");
	let mut agent_bindings = vec![
		json!({"kind":"tool","target":{"registry_node":f.config.node_id,"id":"http","version":"1.0.0"},"alias":"plugin_0","narrow":{}}),
	];
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let authorization = Authorization {
		pool: f.store.pool.clone(),
	};
	let approve = async |id: &str| {
		authorization
			.set_catalog(
				"acme",
				&EntityRef {
					id: id.into(),
					version: "1.0.0".into(),
				},
				0,
				true,
				"deployment_administrator",
			)
			.await
			.unwrap();
	};
	if semantic {
		let memory = json!({"id":"semantic-memory","version":"1.0.0","kind":"memory","name":{"en":"Semantic Memory"},"description":{"en":"Explicit retrieval opt-in"},"schema":{},"config":{"schema_version":1,"source":{"adapter":"semantic_memory"}}});
		f.registry
			.register(&tenant, serde_json::from_value(memory).unwrap())
			.await
			.unwrap();
		approve("semantic-memory").await;
		agent_bindings.push(json!({"kind":"memory","target":{"registry_node":f.config.node_id,"id":"semantic-memory","version":"1.0.0"},"narrow":{}}));
	}
	// The model fixture is a Keyless endpoint on loopback (ADR 0036). An
	// unreachable fixture address stays permitted so it keeps failing to connect.
	f.store.keyless.permit_origin_of(endpoint).unwrap();
	for (kind, id, config) in [
		(
			"model",
			"model",
			json!({"provider":"openrouter","model_id":"fixture","endpoint":format!("{endpoint}/v1"),"context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}),
		),
		(
			"tool",
			"http",
			json!({"registry_node":f.config.node_id,"provider":"integration.http@1","operation":"invoke","default_alias":"plugin_0","tier":"integration","transport":{"transport":"http","endpoint":format!("{endpoint}/effect"),"replay":"idempotent"}}),
		),
		(
			"agent",
			"research",
			json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Test approved work","bindings":agent_bindings,"remove_default":if files {json!([])} else {json!(["file_search","file_read"])} }),
		),
	] {
		let entry = json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"fixture"},"capabilities":[],"languages":["en"],"schema":{"type":"object"},"config":config});
		f.registry
			.register(&tenant, serde_json::from_value(entry).unwrap())
			.await
			.unwrap();
		approve(id).await;
	}
	let (status, credential) = request(
		app,
		deployment_administrator,
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
	// A child process must reach the same Keyless fixtures (ADR 0036).
	settings.keyless_endpoints.permitted_private_origins = f.store.keyless.permitted_origins();
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

/// Every migration of the installed build: Community's, then the build's own.
pub async fn native_migrations() -> Vec<reinhardt::db::migrations::Migration> {
	let server = PathBuf::from(aidash_server::bootstrap::build::Build::COMMUNITY.manifest_dir);
	let mut migrations = Vec::new();
	for root in aidash_server::bootstrap::build::current().migration_roots(&server) {
		migrations.extend(
			FilesystemSource::new(root)
				.all_migrations()
				.await
				.expect("load the native migration graph"),
		);
	}
	migrations
}

/// A Store on the server role's URL, after migrating the same database as its owner.
#[allow(dead_code)]
pub async fn native_store(database_url: &str, node: &str) -> Store {
	let owner = sqlx::PgPool::connect(&postgres::with_login(database_url, postgres::OWNER))
		.await
		.unwrap();
	let connection = DatabaseConnection::new(Arc::new(PostgresBackend::new(owner.clone())));
	let migrations = native_migrations().await;
	postgres::apply_migrations(connection, &migrations).await;
	owner.close().await;
	let pool = sqlx::PgPool::connect(database_url).await.unwrap();
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
	/// The server role's pool, which row-level security binds.
	pub pool: sqlx::PgPool,
	/// The server role's URL.
	pub url: String,
	/// The owner's URL, for migrations and fixture-only DDL.
	#[allow(dead_code)]
	pub owner_url: String,
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
	/// The disposable services that also bound this runtime's fixture stages.
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
pub fn execution_database(
	test_environment: EnvironmentFuture,
	#[default("execution")] prefix: &str,
	#[default(12)] max_connections: u32,
) -> DatabaseFuture {
	let prefix = prefix.to_owned();
	async move {
		let environment = test_environment.await;

		let database = format!("{prefix}_{}", Uuid::new_v4().simple());
		// The preserved baseline names public explicitly. Isolate databases, not
		// search paths. TestEnvironment owns the container and every database, so
		// unwinding also removes fixtures that never reach explicit cleanup.
		// Each stage names the database, which the PostgreSQL snapshot of a
		// hang lists for every session connected to it (#320).
		let logins = environment
			.bounded_setup(
				format!("create the fixture database {database}"),
				postgres::prepare_database(
					&environment.database_url,
					&database,
					postgres::PgroongaRecovery::Off,
				),
			)
			.await;
		// Migrations run as the owner; everything else uses the server role.
		environment
			.bounded_setup(format!("migrate the fixture database {database}"), async {
				let owner = PgPoolOptions::new()
					.max_connections(2)
					.connect(&logins.owner_url)
					.await
					.unwrap();
				let migrations = native_migrations().await;
				postgres::apply_migrations(
					DatabaseConnection::new(Arc::new(PostgresBackend::new(owner.clone()))),
					&migrations,
				)
				.await;
				owner.close().await;
			})
			.await;
		let url = logins.server_url;
		let options = PgConnectOptions::from_str(&url)
			.unwrap()
			.application_name(&database);
		let pool = environment
			.bounded_setup(
				format!("connect the server pool to {database}"),
				PgPoolOptions::new()
					.max_connections(max_connections)
					.connect_with(options),
			)
			.await
			.unwrap();
		Arc::new(ExecutionDatabase {
			pool,
			url,
			owner_url: logins.owner_url,
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
		// The deterministic research fixture provider never leaves the Node.
		aidash_server::research::configure(
			&serde_json::from_value(json!({"page_reading": false, "fixture": true, "brave": null}))
				.unwrap(),
			&reinhardt::conf::settings::profile::Profile::Development,
		)
		.unwrap();

		let store = database
			.environment
			.bounded_setup(
				format!("open the Store on {}", database.schema),
				Store::from_pool(database.pool.clone(), "aidash://execution-test".into()),
			)
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
				api_token: "deployment-administrator-execution-fixture".into(),
				web_dir: "web/dist".into(),
				lease_seconds: 30,
				tenancy: aidash_domain::identity::tenancy::TenancyRules::of(
					aidash_domain::offering::Offering::SelfHosted,
				),
				oidc: None,
				sign_in: None,
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
