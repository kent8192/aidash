//! Controlled native peer responses and an observer around the production routes.
use crate::endpoint::EndpointFixture;
use crate::execution_fixtures::{ExecutionFixture, execution};
use aidash_server::{apps::federation::peer::models::Peer, registry::Entry, routes};
use async_trait::async_trait;
use futures_util::{
	FutureExt,
	future::{LocalBoxFuture, Shared},
};
use reinhardt::db::orm::Model;
use reinhardt::di::{DiError, DiResult, Injectable};
use reinhardt::http::{Handler, Middleware, ViewResult};
use reinhardt::test::fixtures::injection_context;
use reinhardt::test::fixtures::server::{TestServerGuard, test_server_guard};
use reinhardt::{InjectionContext, Request, Response, ServerRouter, post};
use rstest::fixture;
use serde_json::Value;
use std::sync::{
	Arc,
	atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::{Mutex, Notify, mpsc};

pub const PEER_ENV: &str = "AIDASH_SECRET_OUTBOUND_DISCOVERY_FIXTURE";
pub const PEER_SECRET: &str = "outbound-discovery-fixture-0123456789-ABCDEFGHIJKLMNOPQRSTUVWXYZ";

pub struct ObservedRequest {
	pub authorization: String,
	pub source: String,
	pub protocol: String,
	pub body: Value,
}

struct ObserveDiscovery(mpsc::UnboundedSender<ObservedRequest>);
#[async_trait]
impl Middleware for ObserveDiscovery {
	async fn process(&self, request: Request, next: Arc<dyn Handler>) -> ViewResult<Response> {
		if request.uri.path() == "/federation/v0.1/scoped/discover" {
			let header = |name: &str| {
				request
					.headers
					.get(name)
					.unwrap()
					.to_str()
					.unwrap()
					.to_owned()
			};
			self.0
				.send(ObservedRequest {
					authorization: header("authorization"),
					source: header("x-aidash-node"),
					protocol: header("x-aidash-protocol"),
					body: serde_json::from_slice(request.body()).unwrap(),
				})
				.unwrap();
		}
		next.handle(request).await
	}
}

pub async fn persist_peer(app: &EndpointFixture, node: &str, url: &str) {
	let record = Peer::build()
		.node_id(node)
		.endpoint(url)
		.credential_env(PEER_ENV)
		.protocol_version("0.2")
		.enabled(true)
		.finish();
	Peer::objects()
		.create_with_conn(&mut app.database.lease.handle(), &record)
		.await
		.unwrap();
}

pub struct DiscoveryPair {
	pub source: ExecutionFixture,
	pub destination: ExecutionFixture,
	pub observed: mpsc::UnboundedReceiver<ObservedRequest>,
	pub _peer_server: Arc<TestServerGuard>,
}

pub type ExecutionFuture = Shared<LocalBoxFuture<'static, Arc<Mutex<Option<ExecutionFixture>>>>>;
#[fixture]
fn ready_execution(
	execution: impl std::future::Future<Output = ExecutionFixture> + 'static,
) -> ExecutionFuture {
	let execution = Box::pin(execution);
	async move { Arc::new(Mutex::new(Some(execution.await))) }
		.boxed_local()
		.shared()
}
#[fixture]
fn source_execution(
	#[from(execution)]
	#[with("aidash://source")]
	source: impl std::future::Future<Output = ExecutionFixture> + 'static,
) -> ExecutionFuture {
	let source = Box::pin(source);
	async move { Arc::new(Mutex::new(Some(source.await))) }
		.boxed_local()
		.shared()
}
#[fixture]
fn destination_execution(
	#[from(execution)]
	#[with("aidash://destination")]
	destination: impl std::future::Future<Output = ExecutionFixture> + 'static,
) -> ExecutionFuture {
	let destination = Box::pin(destination);
	async move { Arc::new(Mutex::new(Some(destination.await))) }
		.boxed_local()
		.shared()
}
pub struct DiscoveryObservation {
	sender: mpsc::UnboundedSender<ObservedRequest>,
	received: Mutex<Option<mpsc::UnboundedReceiver<ObservedRequest>>>,
}
#[fixture]
fn observation() -> Arc<DiscoveryObservation> {
	let (sender, received) = mpsc::unbounded_channel();
	Arc::new(DiscoveryObservation {
		sender,
		received: Mutex::new(Some(received)),
	})
}
#[fixture]
fn discovery_router(
	destination_execution: ExecutionFuture,
	observation: Arc<DiscoveryObservation>,
) -> LocalBoxFuture<'static, ServerRouter> {
	async move {
		let destination = destination_execution.await;
		let destination = destination.lock().await;
		routes()
			.with_di_context(destination.as_ref().unwrap().app.context.clone())
			.with_middleware(ObserveDiscovery(observation.sender.clone()))
			.into_server()
	}
	.boxed_local()
}
#[fixture]
fn discovery_server(
	#[from(destination_execution)] _destination: ExecutionFuture,
	#[from(observation)] _observation: Arc<DiscoveryObservation>,
	#[from(discovery_router)]
	#[with(_destination.clone(),_observation.clone())]
	router: LocalBoxFuture<'static, ServerRouter>,
) -> LocalBoxFuture<'static, Arc<TestServerGuard>> {
	// reinhardt-web#6658/#6673: own the native guard and serve production routes directly.
	async move { Arc::new(test_server_guard(router.await).await) }.boxed_local()
}
#[fixture]
pub fn discovery_pair(
	source_execution: ExecutionFuture,
	destination_execution: ExecutionFuture,
	observation: Arc<DiscoveryObservation>,
	#[from(discovery_server)]
	#[with(destination_execution.clone(),observation.clone())]
	server: LocalBoxFuture<'static, Arc<TestServerGuard>>,
) -> LocalBoxFuture<'static, DiscoveryPair> {
	async move {
		let source = source_execution.await.lock().await.take().unwrap();
		let destination = destination_execution.await;
		let server = server.await;
		let destination = destination.lock().await.take().unwrap();
		persist_peer(
			&source.app,
			&destination.app.runtime.config.node_id,
			&server.url,
		)
		.await;
		persist_peer(
			&destination.app,
			&source.app.runtime.config.node_id,
			&source.app.server.url,
		)
		.await;
		DiscoveryPair {
			source,
			destination,
			observed: observation.received.lock().await.take().unwrap(),
			_peer_server: server,
		}
	}
	.boxed_local()
}

#[derive(Clone)]
pub struct DiscoveryReply {
	entry: Entry,
	mode: Arc<AtomicUsize>,
	pause: Arc<AtomicBool>,
	started: Arc<Notify>,
	release: Arc<Notify>,
}
#[async_trait]
impl Injectable for DiscoveryReply {
	async fn inject(context: &InjectionContext) -> DiResult<Self> {
		context
			.get_singleton::<Self>()
			.map(|state| (*state).clone())
			.ok_or_else(|| DiError::NotFound("discovery fixture reply".into()))
	}
}
#[post("/federation/v0.1/scoped/discover")]
async fn discover(#[inject] state: DiscoveryReply) -> ViewResult<Response> {
	state.started.notify_one();
	if state.pause.load(Ordering::SeqCst) {
		state.release.notified().await;
	}
	let mut entry = state.entry;
	let entries = match state.mode.load(Ordering::SeqCst) {
		1 => vec![entry.clone(), entry],
		2 => {
			entry.kind = "tool".into();
			vec![entry]
		}
		3 => {
			entry.version = "not-a-version".into();
			vec![entry]
		}
		_ => vec![entry],
	};
	Response::ok().with_json(&entries)
}

pub struct MockDiscovery {
	pub execution: ExecutionFixture,
	pub mode: Arc<AtomicUsize>,
	pub pause: Arc<AtomicBool>,
	pub started: Arc<Notify>,
	pub release: Arc<Notify>,
	pub _server: Arc<TestServerGuard>,
}
impl Drop for MockDiscovery {
	fn drop(&mut self) {
		self.release.notify_one();
	}
}

pub type ReplyFuture = Shared<LocalBoxFuture<'static, DiscoveryReply>>;
#[fixture]
fn discovery_reply(ready_execution: ExecutionFuture) -> ReplyFuture {
	async move {
		let execution = ready_execution.await;
		let execution = execution.lock().await;
		DiscoveryReply {
			entry: execution
				.as_ref()
				.unwrap()
				.app
				.runtime
				.registry
				.get("research", "1.0.0")
				.await
				.unwrap(),
			mode: Arc::new(AtomicUsize::new(0)),
			pause: Arc::new(AtomicBool::new(false)),
			started: Arc::new(Notify::new()),
			release: Arc::new(Notify::new()),
		}
	}
	.boxed_local()
	.shared()
}
#[fixture]
fn mock_router(
	discovery_reply: ReplyFuture,
	injection_context: InjectionContext,
) -> LocalBoxFuture<'static, ServerRouter> {
	async move {
		injection_context.set_singleton(discovery_reply.await);
		ServerRouter::new()
			.endpoint(discover)
			.with_di_context(Arc::new(injection_context))
	}
	.boxed_local()
}
#[fixture]
fn mock_server(
	#[from(discovery_reply)] _state: ReplyFuture,
	#[from(mock_router)]
	# [with(_state.clone())]
	router: LocalBoxFuture<'static, ServerRouter>,
) -> LocalBoxFuture<'static, Arc<TestServerGuard>> {
	async move { Arc::new(test_server_guard(router.await).await) }.boxed_local()
}
#[fixture]
pub fn mock_discovery(
	ready_execution: ExecutionFuture,
	#[from(discovery_reply)]
	#[with(ready_execution.clone())]
	state: ReplyFuture,
	#[from(mock_server)]
	#[with(state.clone())]
	server: LocalBoxFuture<'static, Arc<TestServerGuard>>,
) -> LocalBoxFuture<'static, MockDiscovery> {
	async move {
		let state = state.await;
		let server = server.await;
		let execution = ready_execution.await.lock().await.take().unwrap();
		persist_peer(&execution.app, "aidash://untrusted-peer", &server.url).await;
		MockDiscovery {
			execution,
			mode: state.mode,
			pause: state.pause,
			started: state.started,
			release: state.release,
			_server: server,
		}
	}
	.boxed_local()
}
