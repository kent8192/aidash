//! Compose native Reinhardt fixtures with Aidash's production routes.
use crate::native_database::{DatabaseFixture, database};
#[path = "settings.rs"]
mod settings;
use aidash_server::{bootstrap, federation::Federation};
use reinhardt::InjectionContext;
use reinhardt::test::fixtures::server::{TestServerGuard, test_server_guard};
use reinhardt::test::fixtures::{api_client_from_url, injection_context};
use reinhardt::test::{APIClient, TestResponse};
use rstest::fixture;
use serde_json::{Value, json};
pub use settings::settings_for;
use std::sync::Arc;

#[allow(dead_code)] // Each app suite consumes a different subset of the shared fixture.
pub struct EndpointFixture {
	#[allow(dead_code)] // Stream revocation tests control polling through the same route context.
	pub context: Arc<InjectionContext>,
	pub operator: APIClient,
	pub anonymous: APIClient,
	pub runtime: Federation,
	pub server: TestServerGuard,
	pub database: DatabaseFixture,
}

#[fixture]
pub async fn endpoint(
	#[default("aidash://endpoint-test")] node_id: &'static str,
	#[future] database: DatabaseFixture,
	injection_context: InjectionContext,
) -> EndpointFixture {
	let _ = tracing_subscriber::fmt()
		.with_test_writer()
		.with_env_filter(
			tracing_subscriber::EnvFilter::try_from_default_env()
				.unwrap_or_else(|_| "aidash=debug".into()),
		)
		.try_init();
	let database = database.await;
	let mut settings = settings_for(&database.url);
	settings.node.node_id = node_id.into();
	let runtime = bootstrap::initialize(&injection_context, &settings, database.connection.clone())
		.await
		.expect("initialize native application services");
	let context = Arc::new(injection_context);
	let router = aidash_server::routes()
		.with_di_context(context.clone())
		.into_server();
	let server = test_server_guard(router).await;
	let operator = api_client_from_url(&server.url);
	operator
		.set_header(
			"Authorization",
			&format!("Bearer {}", runtime.config.api_token),
		)
		.await
		.unwrap();
	let anonymous = api_client_from_url(&server.url);
	EndpointFixture {
		context,
		operator,
		anonymous,
		runtime,
		server,
		database,
	}
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
pub async fn subject(fixture: &EndpointFixture, name: &str) -> APIClient {
	let body = json!({"expected_revision":0,"bundle":{"tenant":"endpoint","subjects":{name:{"kind":"user"}},"policies":[{"id":"test-work","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]}});
	assert_json(
		fixture
			.operator
			.post("/api/authorization/endpoint", &body, "json")
			.await
			.unwrap(),
		200,
	);
	let issued = assert_json(
		fixture
			.operator
			.post(
				"/api/authorization/endpoint/credentials",
				&json!({"subject":name}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let client = api_client_from_url(&fixture.server.url);
	client
		.set_header(
			"Authorization",
			&format!("Bearer {}", issued["token"].as_str().unwrap()),
		)
		.await
		.unwrap();
	client
}
