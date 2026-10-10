//! A running production router for fixtures that customize their runtime.
use super::{RuntimeFixture, RuntimeFuture, runtime};
use aidash_server::federation::Federation;
use futures_util::{
	FutureExt,
	future::{BoxFuture, Shared},
};
use reinhardt::db::orm::connection::DatabaseConnectionLease;
use reinhardt::test::APIClient;
use reinhardt::test::fixtures::http_client;
use reinhardt::test::fixtures::server::{TestServerGuard, test_server_guard};
use reinhardt::test::fixtures::{api_client_from_url, injection_context};
use reinhardt::{InjectionContext, ServerRouter};
use rstest::fixture;
use std::sync::Arc;
use std::{future::Future, pin::Pin};

#[path = "settings.rs"]
mod settings;
#[allow(unused_imports)] // Process integration targets also serialize their fixture settings.
pub(crate) use settings::{process_settings, settings_for};

#[derive(Clone)]
pub struct TestApplication {
	pub server: Arc<TestServerGuard>,
	#[allow(dead_code)] // Lifecycle tests register their shutdown coordinator here.
	pub context: Arc<InjectionContext>,
	router: Arc<ServerRouter>,
	#[allow(dead_code)]
	// Only streaming request producers and raw transport cases use this client.
	pub(crate) raw_http: reqwest::Client,
	#[allow(dead_code)] // Only incremental SSE readers consume this transport.
	pub(crate) streaming_http: reqwest::Client,
	pub(crate) api_http: Arc<APIClient>,
	_fixture_owner: Option<RuntimeFixture>,
}

impl TestApplication {
	#[allow(dead_code)] // Only raw HTTP and streaming clients need an absolute URL.
	pub fn url(&self, path: impl AsRef<str>) -> String {
		format!("{}{}", self.server.url, path.as_ref())
	}
	/// Reuse the declared anonymous client; credentialed clients are separate fixtures.
	#[allow(dead_code)] // Each integration binary consumes different client fixtures.
	pub fn client(&self) -> Arc<APIClient> {
		self.api_http.clone()
	}
}

pub fn application(runtime: Federation) -> Pin<Box<dyn Future<Output = TestApplication> + Send>> {
	Box::pin(application_with(runtime, |router| router))
}

pub async fn application_with(
	runtime: Federation,
	router: impl Fn(ServerRouter) -> ServerRouter,
) -> TestApplication {
	// Production startup admits immutable system declarations before serving routes.
	runtime.registry.seed_system().await.unwrap();
	let _ = tracing_subscriber::fmt()
		.with_test_writer()
		.with_env_filter(
			tracing_subscriber::EnvFilter::try_from_default_env()
				.unwrap_or_else(|_| "aidash=debug".into()),
		)
		.try_init();
	let raw_http = runtime.client.clone();
	let context = injection_context::default();
	let mut settings = settings_for(&runtime.config.database_url);
	settings.node.node_id = runtime.config.node_id.clone();
	settings.node.endpoint = runtime.config.endpoint.clone();
	settings.node.api_token = runtime.config.api_token.clone();
	settings.node.web_dir = runtime.config.web_dir.clone();
	settings.node.lease_seconds = runtime.config.lease_seconds;
	let connection = runtime.store.pool.connection();
	let lease = DatabaseConnectionLease::register(connection).unwrap();
	context.set_singleton(lease.handle());
	context.set_singleton(lease);
	context.set_singleton(settings);
	context.set_singleton(runtime);
	context.set_singleton(reinhardt::di::KeyedFactoryOutput::<
		reinhardt::di::SelfKey<aidash_server::http::Protection>,
		aidash_server::http::Protection,
	>::new(aidash_server::http::Protection::new(
		aidash_server::http::Settings::default(),
	)));
	context.set_singleton(reinhardt::di::KeyedFactoryOutput::<
		reinhardt::di::SelfKey<aidash_server::sse::Service>,
		aidash_server::sse::Service,
	>::new(aidash_server::sse::Service::new(
		aidash_server::sse::Settings::default(),
	)));
	let context = Arc::new(context);
	// Serve production routes directly to preserve their method dispatch and DI.
	// Separate route tables share the same DI services and scenario middleware state.
	let server_router =
		router(aidash_server::routes().into_server()).with_di_context(context.clone());
	let router =
		Arc::new(router(aidash_server::routes().into_server()).with_di_context(context.clone()));
	let server = test_server_guard(server_router).await;
	let api_http = Arc::new(api_client_from_url(&server.url));
	TestApplication {
		raw_http,
		// Act: lifecycle rebuilds retain the baseline stream transport policy.
		streaming_http: streaming_http_client::default(),
		api_http,
		_fixture_owner: None,
		server: Arc::new(server),
		context,
		router,
	}
}

impl TestApplication {
	/// Share the native router with fixed-port peer restart fixtures.
	#[allow(dead_code)] // Only restart and transport fault suites need another listener.
	pub fn native_router(&self) -> Arc<ServerRouter> {
		self.router.clone()
	}

	/// Preserve synthetic socket peers and unpolled response producer ownership.
	#[allow(dead_code)] // Normal endpoint assertions use APIClient instead.
	pub async fn native_oneshot(
		self,
		request: http::Request<bytes::Bytes>,
	) -> reinhardt::Result<reinhardt::Response> {
		use reinhardt::Handler;
		let (parts, body) = request.into_parts();
		let peer = parts
			.extensions
			.get::<std::net::SocketAddr>()
			.copied()
			.unwrap_or(([127, 0, 0, 1], 1).into());
		let request = reinhardt::Request::from_hyper_parts(
			parts.method,
			parts.uri,
			parts.version,
			parts.headers,
			body,
			false,
			Some(peer),
		);
		self.router.handle(request).await
	}
}
#[allow(dead_code)]
pub async fn application_with_settings(
	runtime: Federation,
	settings: aidash_server::http::Settings,
) -> TestApplication {
	application_with_event_streams(
		runtime,
		settings,
		aidash_server::sse::Service::new(aidash_server::sse::Settings::default()),
	)
	.await
}
#[allow(dead_code)]
pub async fn application_with_event_streams(
	runtime: Federation,
	settings: aidash_server::http::Settings,
	service: aidash_server::sse::Service,
) -> TestApplication {
	let application = application(runtime).await;
	application
		.context
		.set_singleton(reinhardt::di::KeyedFactoryOutput::<
			reinhardt::di::SelfKey<aidash_server::http::Protection>,
			aidash_server::http::Protection,
		>::new(aidash_server::http::Protection::new(settings)));
	application
		.context
		.set_singleton(reinhardt::di::KeyedFactoryOutput::<
			reinhardt::di::SelfKey<aidash_server::sse::Service>,
			aidash_server::sse::Service,
		>::new(service));
	application
}

type ContextFuture = Shared<BoxFuture<'static, Arc<InjectionContext>>>;
type RouterFuture = Shared<BoxFuture<'static, Arc<ServerRouter>>>;
type ServerFuture = Shared<BoxFuture<'static, Arc<TestServerGuard>>>;
type ClientFuture = Shared<BoxFuture<'static, Arc<APIClient>>>;
pub type ApplicationFuture = Shared<BoxFuture<'static, ApplicationFixture>>;

#[derive(Clone)]
#[allow(dead_code)] // Integration targets independently consume the runtime or application.
pub struct ApplicationFixture {
	pub application: TestApplication,
	pub runtime: RuntimeFixture,
	pub operator: Arc<APIClient>,
	pub anonymous: Arc<APIClient>,
}

#[fixture]
fn application_context(
	#[default(aidash_server::http::Settings::default())] protection: aidash_server::http::Settings,
	#[default(aidash_server::sse::Service::new(aidash_server::sse::Settings::default()))]
	streams: aidash_server::sse::Service,
	runtime: RuntimeFuture,
	injection_context: InjectionContext,
) -> ContextFuture {
	async move {
		let owner = runtime.await;
		let runtime = owner.federation.clone();
		// Production startup admits immutable system declarations before serving routes.
		runtime.registry.seed_system().await.unwrap();
		let _ = tracing_subscriber::fmt()
			.with_test_writer()
			.with_env_filter(
				tracing_subscriber::EnvFilter::try_from_default_env()
					.unwrap_or_else(|_| "aidash=debug".into()),
			)
			.try_init();
		let context = injection_context;
		let mut settings = settings_for(&runtime.config.database_url);
		settings.node.node_id = runtime.config.node_id.clone();
		settings.node.endpoint = runtime.config.endpoint.clone();
		settings.node.api_token = runtime.config.api_token.clone();
		settings.node.web_dir = runtime.config.web_dir.clone();
		settings.node.lease_seconds = runtime.config.lease_seconds;
		let connection = runtime.store.pool.connection();
		let lease = DatabaseConnectionLease::register(connection).unwrap();
		context.set_singleton(lease.handle());
		context.set_singleton(lease);
		context.set_singleton(settings);
		context.set_singleton(runtime);
		context.set_singleton(reinhardt::di::KeyedFactoryOutput::<
			reinhardt::di::SelfKey<aidash_server::http::Protection>,
			aidash_server::http::Protection,
		>::new(aidash_server::http::Protection::new(protection)));
		context.set_singleton(reinhardt::di::KeyedFactoryOutput::<
			reinhardt::di::SelfKey<aidash_server::sse::Service>,
			aidash_server::sse::Service,
		>::new(streams));
		Arc::new(context)
	}
	.boxed()
	.shared()
}

pub type RouterTransform = Arc<dyn Fn(ServerRouter) -> ServerRouter + Send + Sync>;

#[fixture]
fn application_router(
	#[default(Arc::new(|router| router))] transform: RouterTransform,
	application_context: ContextFuture,
) -> RouterFuture {
	async move {
		Arc::new(
			transform(aidash_server::routes().into_server())
				.with_di_context(application_context.await),
		)
	}
	.boxed()
	.shared()
}

#[fixture]
fn application_server(
	#[default(Arc::new(|router| router))] transform: RouterTransform,
	application_context: ContextFuture,
) -> ServerFuture {
	async move {
		// Compose the shared native guard with direct
		// production routes; share DI and transform state with the oneshot router.
		let router = transform(aidash_server::routes().into_server())
			.with_di_context(application_context.await);
		Arc::new(test_server_guard(router).await)
	}
	.boxed()
	.shared()
}

#[fixture]
fn application_client(
	#[default(false)] operator: bool,
	runtime: RuntimeFuture,
	application_server: ServerFuture,
) -> ClientFuture {
	async move {
		// Share the client after resolving the owned asynchronous server URL.
		let client = api_client_from_url(&application_server.await.url);
		if operator {
			client
				.set_header(
					"Authorization",
					&format!("Bearer {}", runtime.await.federation.config.api_token),
				)
				.await
				.unwrap();
		}
		Arc::new(client)
	}
	.boxed()
	.shared()
}

#[derive(Clone)]
pub struct ApplicationTransport {
	runtime: RuntimeFixture,
	context: Arc<InjectionContext>,
	router: Arc<ServerRouter>,
	server: Arc<TestServerGuard>,
}
pub type TransportFuture = Shared<BoxFuture<'static, ApplicationTransport>>;
#[derive(Clone)]
pub struct ApplicationClients {
	anonymous: Arc<APIClient>,
	operator: Arc<APIClient>,
	raw: reqwest::Client,
	streaming: reqwest::Client,
}
pub type ClientsFuture = Shared<BoxFuture<'static, ApplicationClients>>;

#[fixture]
fn application_transport(
	#[default(aidash_server::http::Settings::default())] _protection: aidash_server::http::Settings,
	#[default(aidash_server::sse::Service::new(aidash_server::sse::Settings::default()))] _streams: aidash_server::sse::Service,
	#[default(Arc::new(|router| router))] _transform: RouterTransform,
	runtime: RuntimeFuture,
	#[from(application_context)]
	#[with(_protection.clone(), _streams.clone(), runtime.clone())]
	context: ContextFuture,
	#[from(application_router)]
	#[with(_transform.clone(), context.clone())]
	router: RouterFuture,
	#[from(application_server)]
	#[with(_transform.clone(), context.clone())]
	server: ServerFuture,
) -> TransportFuture {
	async move {
		ApplicationTransport {
			runtime: runtime.await,
			context: context.await,
			router: router.await,
			server: server.await,
		}
	}
	.boxed()
	.shared()
}
#[fixture]
fn transport_server(application_transport: TransportFuture) -> ServerFuture {
	async move { application_transport.await.server }
		.boxed()
		.shared()
}
#[fixture]
pub fn streaming_http_client() -> reqwest::Client {
	// reinhardt-web#6689: the native HTTP client has a 10s total timeout.
	// Retain the baseline unlimited stream lifetime; delivery Acts have their own deadlines.
	reqwest::Client::builder().build().unwrap()
}
#[fixture]
fn application_clients(
	#[from(application_transport)] _transport: TransportFuture,
	#[from(runtime)] _runtime: RuntimeFuture,
	#[from(transport_server)]
	#[with(_transport.clone())]
	_server: ServerFuture,
	#[from(application_client)]
	#[with(false, _runtime.clone(), _server.clone())]
	anonymous: ClientFuture,
	#[from(application_client)]
	#[with(true, _runtime.clone(), _server.clone())]
	operator: ClientFuture,
	http_client: reqwest::Client,
	streaming_http_client: reqwest::Client,
) -> ClientsFuture {
	async move {
		ApplicationClients {
			anonymous: anonymous.await,
			operator: operator.await,
			raw: http_client,
			streaming: streaming_http_client,
		}
	}
	.boxed()
	.shared()
}
#[fixture]
pub fn native_application(
	#[default(aidash_server::http::Settings::default())] _protection: aidash_server::http::Settings,
	#[default(aidash_server::sse::Service::new(aidash_server::sse::Settings::default()))] _streams: aidash_server::sse::Service,
	#[default(Arc::new(|router| router))] _transform: RouterTransform,
	#[from(runtime)] _runtime: RuntimeFuture,
	#[from(application_transport)]
	#[with(_protection.clone(), _streams.clone(), _transform.clone(), _runtime.clone())]
	_transport: TransportFuture,
	#[from(application_clients)]
	#[with(_transport.clone(), _runtime.clone())]
	_clients: ClientsFuture,
) -> ApplicationFuture {
	async move {
		let transport = _transport.await;
		let clients = _clients.await;
		let application = TestApplication {
			server: transport.server,
			context: transport.context,
			router: transport.router,
			raw_http: clients.raw,
			streaming_http: clients.streaming,
			api_http: clients.anonymous.clone(),
			_fixture_owner: Some(transport.runtime.clone()),
		};
		ApplicationFixture {
			application,
			runtime: transport.runtime,
			operator: clients.operator,
			anonymous: clients.anonymous,
		}
	}
	.boxed()
	.shared()
}

#[fixture]
fn direct_client(native_application: ApplicationFuture) -> ClientFuture {
	async move {
		let application = native_application.await.application;
		// In-process dispatch also preserves unpolled producer ownership.
		let client = APIClient::from_handler(application.native_router());
		// reinhardt-web#6672: from_handler injects a default Origin. Browser cases
		// must supply their exact per-request Origin, including denied origins.
		client.cleanup().await;
		Arc::new(client)
	}
	.boxed()
	.shared()
}

#[fixture]
pub fn direct_application(
	#[default(aidash_server::http::Settings::default())] _protection: aidash_server::http::Settings,
	#[default(aidash_server::sse::Service::new(Default::default()))]
	_streams: aidash_server::sse::Service,
	#[default(Arc::new(|router| router))] _transform: RouterTransform,
	#[from(runtime)] _runtime: RuntimeFuture,
	#[from(native_application)]
	#[with(_protection.clone(), _streams.clone(), _transform.clone(), _runtime.clone())]
	_application: ApplicationFuture,
	#[from(direct_client)]
	#[with(_application.clone())]
	client: ClientFuture,
) -> ApplicationFuture {
	async move {
		let mut application = _application.await;
		application.application.api_http = client.await.clone();
		application.anonymous = application.application.api_http.clone();
		application
	}
	.boxed()
	.shared()
}
