//! Owned native HTTP stubs for external-provider and peer contracts.
#![allow(dead_code)] // Integration targets consume different parts of this shared fixture.
use futures_util::{
	FutureExt,
	future::{BoxFuture, Shared},
};
use reinhardt::http::ViewResult;
use reinhardt::test::fixtures::server::{TestServerGuard, test_server_guard};
use reinhardt::{Handler, Request, Response, ServerRouter};
use rstest::fixture;
use std::{future::Future, sync::Arc};

/// Keep listener and connection cleanup in Reinhardt's fixture guard.
/// Retain router identity while dependent fixtures also inspect its shared state.
#[fixture]
pub async fn upstream(
	#[default(Arc::new(ServerRouter::new()))] router: Arc<ServerRouter>,
) -> TestServerGuard {
	// reinhardt-web#6658: the pinned guard is not a composable rstest fixture.
	let transport = ServerRouter::new()
		.handler_arc("/", router.clone())
		.handler_arc("/{*rest}", router);
	test_server_guard(transport).await
}

// reinhardt-web#6657: adapt closure types until method-aware closure routes exist.
pub fn handler<F, Fut>(method: http::Method, reply: F) -> impl Handler
where
	F: Fn(Request) -> Fut + Send + Sync + 'static,
	Fut: Future<Output = Response> + Send,
{
	StubHandler {
		method: Some(method),
		reply,
	}
}

// A fallback must accept every method while retaining the same closure adapter.
pub fn any_handler<F, Fut>(reply: F) -> impl Handler
where
	F: Fn(Request) -> Fut + Send + Sync + 'static,
	Fut: Future<Output = Response> + Send,
{
	StubHandler {
		method: None,
		reply,
	}
}

struct StubHandler<F> {
	method: Option<http::Method>,
	reply: F,
}

#[async_trait::async_trait]
impl<F, Fut> Handler for StubHandler<F>
where
	F: Fn(Request) -> Fut + Send + Sync,
	Fut: Future<Output = Response> + Send,
{
	async fn handle(&self, request: Request) -> ViewResult<Response> {
		if self
			.method
			.as_ref()
			.is_some_and(|method| request.method != *method)
		{
			return Ok(Response::new(http::StatusCode::METHOD_NOT_ALLOWED));
		}
		Ok((self.reply)(request).await)
	}
}

pub type RouterFuture = Shared<BoxFuture<'static, Arc<ServerRouter>>>;
pub type UpstreamFuture = Shared<BoxFuture<'static, Arc<TestServerGuard>>>;

/// Compose a router whose scenario state depends on asynchronous database setup.
#[fixture]
pub fn async_upstream(
	#[default(async { Arc::new(ServerRouter::new()) }.boxed().shared())] router: RouterFuture,
) -> UpstreamFuture {
	async move {
		let router = router.await;
		// reinhardt-web#6658: the pinned guard has no fixture dependency resolution.
		let transport = ServerRouter::new()
			.handler_arc("/", router.clone())
			.handler_arc("/{*rest}", router);
		Arc::new(test_server_guard(transport).await)
	}
	.boxed()
	.shared()
}

#[fixture]
pub fn hits() -> Arc<std::sync::atomic::AtomicUsize> {
	Arc::new(std::sync::atomic::AtomicUsize::new(0))
}

#[fixture]
pub fn available() -> Arc<std::sync::atomic::AtomicBool> {
	Arc::new(std::sync::atomic::AtomicBool::new(false))
}

#[fixture]
pub fn ready_router(
	#[default(Arc::new(ServerRouter::new()))] router: Arc<ServerRouter>,
) -> RouterFuture {
	async move { router }.boxed().shared()
}

pub type ListenerFuture = Shared<BoxFuture<'static, Arc<tokio::net::TcpListener>>>;

#[fixture]
pub fn fixed_listener() -> ListenerFuture {
	// Peer restart and origin-bound identity tests require an address before routing setup.
	async { Arc::new(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap()) }
		.boxed()
		.shared()
}

pub struct FixedServerGuard {
	pub url: String,
	task: tokio::task::JoinHandle<()>,
}

impl FixedServerGuard {
	/// Native transport primitive; the owning fixture supplies all setup dependencies.
	pub fn spawn(
		listener: Arc<tokio::net::TcpListener>,
		router: Arc<dyn Handler>,
		context: Option<Arc<reinhardt::InjectionContext>>,
	) -> Self {
		let url = format!("http://{}", listener.local_addr().unwrap());
		let task = tokio::spawn(async move {
			let mut connections = tokio::task::JoinSet::new();
			loop {
				tokio::select! {
					accepted = listener.accept() => {
						let (stream,peer) = accepted.unwrap();
						let router = router.clone();
						let context = context.clone();
						connections.spawn(async move { let _ = reinhardt::server::HttpServer::handle_connection(stream,peer,router,context).await; });
					}
					_ = connections.join_next(), if !connections.is_empty() => {}
				}
			}
		});
		Self { url, task }
	}
	pub fn abort(&self) {
		self.task.abort();
	}
	pub async fn stopped(mut self) -> Result<(), tokio::task::JoinError> {
		(&mut self.task).await
	}
}
impl Drop for FixedServerGuard {
	fn drop(&mut self) {
		self.task.abort();
	}
}

#[fixture]
pub fn fixed_upstream(
	#[default(None)] context: Option<Arc<reinhardt::InjectionContext>>,
	fixed_listener: ListenerFuture,
	#[from(ready_router)] router: RouterFuture,
) -> BoxFuture<'static, FixedServerGuard> {
	async move { FixedServerGuard::spawn(fixed_listener.await, router.await, context) }.boxed()
}

/// Resolve the router and disposable transport as one natural provider dependency.
#[fixture]
pub fn provider_transport(
	#[default(Arc::new(ServerRouter::new()))] router: Arc<ServerRouter>,
	#[from(ready_router)]
	#[with(router.clone())]
	_ready: RouterFuture,
	#[from(async_upstream)]
	#[with(_ready.clone())]
	server: UpstreamFuture,
) -> UpstreamFuture {
	let _ = (router, _ready);
	server
}
