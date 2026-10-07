//! A running production router for fixtures that customize their runtime.
use aidash_server::federation::Federation;
use reinhardt::db::orm::connection::DatabaseConnectionLease;
use reinhardt::test::APIClient;
use reinhardt::test::fixtures::server::{TestServerGuard, test_server_guard};
use reinhardt::test::fixtures::{api_client_from_url, injection_context};
use reinhardt::{InjectionContext, ServerRouter};
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
}

impl TestApplication {
	#[allow(dead_code)] // Only raw HTTP and streaming clients need an absolute URL.
	pub fn url(&self, path: impl AsRef<str>) -> String {
		format!("{}{}", self.server.url, path.as_ref())
	}
	/// Separate clients keep concurrent credentials and cookies isolated.
	pub fn client(&self) -> APIClient {
		api_client_from_url(&self.server.url)
	}
}

pub fn application(runtime: Federation) -> Pin<Box<dyn Future<Output = TestApplication> + Send>> {
	Box::pin(application_with(runtime, |router| router))
}

pub async fn application_with(
	runtime: Federation,
	router: impl FnOnce(ServerRouter) -> ServerRouter,
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
	let router =
		Arc::new(router(aidash_server::routes().into_server()).with_di_context(context.clone()));
	// Share the same native router and DI state with the disposable HTTP fixture
	// and with body-ownership tests that deliberately never poll a response.
	let transport = ServerRouter::new()
		.handler_arc("/", router.clone())
		.handler_arc("/{*rest}", router.clone())
		.with_di_context(context.clone());
	let server = test_server_guard(transport).await;
	TestApplication {
		server: Arc::new(server),
		context,
		router,
	}
}

#[allow(dead_code)] // Only federation suites advertise their ephemeral listener.
pub async fn peer_application(runtime: &mut Federation) -> TestApplication {
	let app = application(runtime.clone()).await;
	runtime.config.endpoint = app.server.url.clone();
	app.context.set_singleton(runtime.clone());
	app
}

impl TestApplication {
	/// Execute Reinhardt itself; preserve producer ownership for unpolled SSE tests.
	#[allow(dead_code)]
	pub async fn oneshot(
		self,
		request: http::Request<axum::body::Body>,
	) -> Result<axum::response::Response, Box<dyn std::error::Error + Send + Sync>> {
		use reinhardt::Handler;
		let (parts, body) = request.into_parts();
		let peer = parts
			.extensions
			.get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
			.map(|address| address.0)
			.unwrap_or(([127, 0, 0, 1], 1).into());
		let bytes = axum::body::to_bytes(body, 16 * 1024 * 1024).await?;
		let request = reinhardt::Request::from_hyper_parts(
			parts.method,
			parts.uri,
			parts.version,
			parts.headers,
			bytes,
			false,
			Some(peer),
		);
		let mut response = self.router.handle(request).await?;
		let body = if let Some(stream) = response.take_stream_body() {
			axum::body::Body::from_stream(stream)
		} else if let Some(file) = response.file_body().cloned() {
			axum::body::Body::from_stream(
				async_stream::stream! {let mut position=0;while position<file.len() {let source=file.clone();match tokio::task::spawn_blocking(move||source.read_chunk(position,64*1024)).await.map_err(std::io::Error::other).and_then(|result|result) {Ok(bytes)=>{position+=bytes.len() as u64;yield Ok::<_,std::io::Error>(bytes);},Err(error)=>{yield Err(error);break;}}}},
			)
		} else {
			axum::body::Body::from(response.body)
		};
		let mut result = axum::response::Response::new(body);
		*result.status_mut() = response.status;
		*result.headers_mut() = response.headers;
		Ok(result)
	}
	/// A test-only transport adapter for existing fluent assertions. All routing,
	/// extraction, authorization, and streaming come from the native handler above.
	#[allow(dead_code)]
	pub fn test_transport(&self) -> axum::Router {
		let application = self.clone();
		axum::Router::new().fallback(move |request: axum::extract::Request| {
			let application = application.clone();
			async move {
				application
					.oneshot(request)
					.await
					.expect("native test response")
			}
		})
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
