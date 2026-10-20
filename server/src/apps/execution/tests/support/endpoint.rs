//! Compose native Reinhardt fixtures with Aidash's production routes.
use crate::native_database::{DatabaseFixture, DatabaseFuture, database};
use futures_util::{
	FutureExt,
	future::{BoxFuture, Shared},
};
#[path = "settings.rs"]
mod settings;
#[path = "tenant_roots.rs"]
mod tenant_roots;
use aidash_server::{bootstrap, federation::Federation};
use reinhardt::InjectionContext;
use reinhardt::test::fixtures::server::{TestServerGuard, test_server_guard};
use reinhardt::test::fixtures::{api_client_from_url, injection_context};
use reinhardt::test::{APIClient, TestResponse};
use rstest::fixture;
use serde_json::{Value, json};
pub use settings::settings_for;
use std::sync::Arc;
#[allow(unused_imports)] // Only suites that create Tenants under fixed names use these.
pub use tenant_roots::{
	admitted_run, approve_agent, local_run, remote_run, remote_run_in, replace_policy, tenant_root,
	tenant_root_in, tenant_scope, tenant_token,
};

#[allow(dead_code)] // Each app suite consumes a different subset of the shared fixture.
#[derive(Clone)]
pub struct EndpointFixture {
	#[allow(dead_code)] // Stream revocation tests control polling through the same route context.
	pub context: Arc<InjectionContext>,
	pub router: Arc<reinhardt::ServerRouter>,
	pub deployment_administrator: Arc<APIClient>,
	pub anonymous: Arc<APIClient>,
	pub runtime: Federation,
	pub server: Arc<TestServerGuard>,
	pub database: DatabaseFixture,
}

pub type EndpointFuture = Shared<BoxFuture<'static, EndpointFixture>>;
type RuntimeFuture = Shared<BoxFuture<'static, Arc<EndpointRuntime>>>;
type RouterFuture = Shared<BoxFuture<'static, Arc<reinhardt::ServerRouter>>>;
type ServerFuture = Shared<BoxFuture<'static, Arc<TestServerGuard>>>;
pub type ClientFuture = Shared<BoxFuture<'static, Arc<APIClient>>>;

struct EndpointRuntime {
	context: Arc<InjectionContext>,
	runtime: Federation,
}

#[fixture]
fn endpoint_runtime(
	#[default("aidash://endpoint-test")] node_id: &'static str,
	database: DatabaseFuture,
	injection_context: InjectionContext,
) -> RuntimeFuture {
	async move {
		let _ = tracing_subscriber::fmt()
			.with_test_writer()
			.with_env_filter(
				tracing_subscriber::EnvFilter::try_from_default_env()
					.unwrap_or_else(|_| "aidash=debug".into()),
			)
			.try_init();
		let database = database.await;
		let mut settings = settings_for(&database.url);
		settings.deployment.id = node_id.into();
		let runtime =
			bootstrap::initialize(&injection_context, &settings, database.connection.clone())
				.await
				.expect("initialize native application services");
		Arc::new(EndpointRuntime {
			context: Arc::new(injection_context),
			runtime,
		})
	}
	.boxed()
	.shared()
}

#[fixture]
fn endpoint_router(endpoint_runtime: RuntimeFuture) -> RouterFuture {
	async move {
		Arc::new(
			aidash_server::routes()
				.with_di_context(endpoint_runtime.await.context.clone())
				.into_server(),
		)
	}
	.boxed()
	.shared()
}

#[fixture]
fn endpoint_server(endpoint_runtime: RuntimeFuture) -> ServerFuture {
	async move {
		// Serve production
		// routes directly, sharing DI with the separate in-process router fixture.
		let router = aidash_server::routes()
			.with_di_context(endpoint_runtime.await.context.clone())
			.into_server();
		Arc::new(test_server_guard(router).await)
	}
	.boxed()
	.shared()
}

#[fixture]
fn endpoint_client(
	#[default(false)] deployment_administrator: bool,
	endpoint_runtime: RuntimeFuture,
	endpoint_server: ServerFuture,
) -> ClientFuture {
	async move {
		let client = api_client_from_url(&endpoint_server.await.url);
		if deployment_administrator {
			client
				.set_header(
					"Authorization",
					&format!("Bearer {}", endpoint_runtime.await.runtime.config.api_token),
				)
				.await
				.unwrap();
		}
		Arc::new(client)
	}
	.boxed()
	.shared()
}

#[fixture]
pub fn endpoint(
	#[default("aidash://endpoint-test")] _node_id: &'static str,
	database: DatabaseFuture,
	#[from(endpoint_runtime)]
	#[with(_node_id, database.clone())]
	runtime: RuntimeFuture,
	#[from(endpoint_router)]
	#[with(runtime.clone())]
	_router: RouterFuture,
	#[from(endpoint_server)]
	#[with(runtime.clone())]
	server: ServerFuture,
	#[from(endpoint_client)]
	#[with(true, runtime.clone(), server.clone())]
	deployment_administrator: ClientFuture,
	#[from(endpoint_client)]
	#[with(false, runtime.clone(), server.clone())]
	anonymous: ClientFuture,
) -> EndpointFuture {
	async move {
		let runtime = runtime.await;
		EndpointFixture {
			context: runtime.context.clone(),
			router: _router.await,
			deployment_administrator: deployment_administrator.await,
			anonymous: anonymous.await,
			runtime: runtime.runtime.clone(),
			server: server.await,
			database: database.await,
		}
	}
	.boxed()
	.shared()
}

pub fn assert_json(response: TestResponse, status: u16) -> Value {
	assert_eq!(response.status_code(), status, "{}", response.text());
	assert_eq!(response.content_type(), Some("application/json"));
	response.json_value().expect("JSON endpoint response")
}

/// Standard JSON extraction preserves the previous backend's text rejection.
#[allow(dead_code)] // Only contract suites with malformed typed requests need this assertion.
pub fn assert_json_rejection(response: TestResponse, status: u16) {
	assert_eq!(response.status_code(), status, "{}", response.text());
	assert_eq!(response.content_type(), Some("text/plain; charset=utf-8"));
	let prefix = match status {
		422 => "Failed to deserialize the JSON body into the target type: ",
		400 => "Failed to parse the request body as JSON: ",
		415 => "Expected request with `Content-Type: application/json`",
		_ => panic!("unsupported JSON rejection status: {status}"),
	};
	assert!(response.text().starts_with(prefix), "{}", response.text());
}

#[allow(dead_code)] // Used by workspace-oriented endpoint suites.
pub async fn workspace(client: &APIClient, title: &str) -> Value {
	assert_json(
		client
			.post(
				"/api/workspaces",
				&json!({"title": title, "goal": "Endpoint integration test"}),
				"json",
			)
			.await
			.unwrap(),
		200,
	)
}

#[allow(dead_code)] // Used by scoped-credential endpoint suites.
pub async fn subject(
	fixture: &EndpointFixture,
	name: &str,
	client: Arc<APIClient>,
) -> Arc<APIClient> {
	tenant_root(&fixture.runtime, "endpoint").await;
	let body = json!({"expected_revision":0,"bundle":{"tenant":"endpoint","subjects":{name:{"kind":"user"}},"policies":[{"id":"test-work","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]}});
	assert_json(
		fixture
			.deployment_administrator
			.post("/api/authorization/endpoint", &body, "json")
			.await
			.unwrap(),
		200,
	);
	let issued = assert_json(
		fixture
			.deployment_administrator
			.post(
				"/api/authorization/endpoint/credentials",
				&json!({"subject":name}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	client
		.set_header(
			"Authorization",
			&format!("Bearer {}", issued["token"].as_str().unwrap()),
		)
		.await
		.unwrap();
	client
}

#[allow(dead_code)]
pub async fn tenant_subject(fixture: &EndpointFixture, name: &str) -> Arc<APIClient> {
	subject(
		fixture,
		name,
		Arc::new(api_client_from_url(&fixture.server.url)),
	)
	.await
}

#[allow(dead_code)]
pub async fn tenant_member(fixture: &EndpointFixture) -> (Arc<APIClient>, String) {
	let token = tenant_token(&fixture.runtime.store, "acme").await;
	let client = api_client_from_url(&fixture.server.url);
	client
		.set_header("Authorization", &format!("Bearer {token}"))
		.await
		.unwrap();
	(Arc::new(client), token)
}

#[allow(dead_code)]
pub async fn approve(fixture: &EndpointFixture, tenant: &str, id: &str, version: &str) {
	aidash_server::authorization::Authorization {
		pool: fixture.runtime.store.pool.clone(),
	}
	.set_catalog(
		tenant,
		&aidash_server::registry::EntityRef {
			id: id.into(),
			version: version.into(),
		},
		0,
		true,
		"deployment_administrator",
	)
	.await
	.expect("the catalog approval is recorded");
}

#[fixture]
pub fn anonymous_client(endpoint: EndpointFuture) -> ClientFuture {
	async move { Arc::new(api_client_from_url(&endpoint.await.server.url)) }
		.boxed()
		.shared()
}

/// Positive native Run fixtures use an admitted graph, including Node defaults.
#[allow(dead_code)]
pub async fn register_fixture_agent(fixture: &EndpointFixture, id: &str) {
	let model = format!("{id}-fixture-model");
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	tenant_root_in(fixture.runtime.store.pool.fixture_driver(), "acme").await;
	fixture.runtime.registry.seed(&tenant).await.unwrap();
	for (kind, key, config) in [
		(
			"model",
			model.as_str(),
			json!({"provider":"openrouter","model_id":"fixture","endpoint":"http://127.0.0.1:1/v1","context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}),
		),
		(
			"agent",
			id,
			json!({"schema_version":1,"model":{"id":model,"version":"1.0.0"},"instructions":"Native Run fixture","bindings":[],"remove_default":[]}),
		),
	] {
		fixture.runtime.registry.register(&tenant, serde_json::from_value(json!({"id":key,"version":"1.0.0","kind":kind,"name":{"en":key},"description":{"en":"Native Run fixture"},"schema":{},"config":config})).unwrap()).await.unwrap();
	}
}
