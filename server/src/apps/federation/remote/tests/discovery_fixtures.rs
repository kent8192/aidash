//! Controlled native peer responses and an observer around the production routes.
use crate::endpoint::EndpointFixture;
use crate::execution_fixtures::{ExecutionFixture, execution};
use aidash_server::{apps::federation::peer::models::Peer, registry::Entry, routes};
use async_trait::async_trait;
use reinhardt::db::orm::Model;
use reinhardt::di::{DiError, DiResult, Injectable};
use reinhardt::http::{Handler, Middleware, ViewResult};
use reinhardt::test::fixtures::injection_context;
use reinhardt::test::fixtures::server::{TestServerGuard, test_server_guard};
use reinhardt::{InjectionContext, Request, Response, ServerRouter, post};
use rstest::fixture;
use serde_json::Value;
use std::{
	future::Future,
	sync::{
		Arc,
		atomic::{AtomicBool, AtomicUsize, Ordering},
	},
};
use tokio::sync::{Notify, mpsc};

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
		.protocol_version("0.1")
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
	pub _peer_server: TestServerGuard,
}

#[fixture]
pub fn discovery_pair(
	#[future]
	#[from(execution)]
	#[with("aidash://source")]
	source: ExecutionFixture,
	#[future]
	#[from(execution)]
	#[with("aidash://destination")]
	destination: ExecutionFixture,
) -> impl Future<Output = DiscoveryPair> {
	let source = Box::pin(source);
	let destination = Box::pin(destination);
	async move {
		let source = source.await;
		let destination = destination.await;
		let (sender, observed) = mpsc::unbounded_channel();
		let router = routes()
			.with_di_context(destination.app.context.clone())
			.with_middleware(ObserveDiscovery(sender))
			.into_server();
		let peer_server = test_server_guard(router).await;
		persist_peer(
			&source.app,
			&destination.app.runtime.config.node_id,
			&peer_server.url,
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
			observed,
			_peer_server: peer_server,
		}
	}
}

#[derive(Clone)]
struct DiscoveryReply {
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
	pub _server: TestServerGuard,
}
impl Drop for MockDiscovery {
	fn drop(&mut self) {
		self.release.notify_one();
	}
}

#[fixture]
pub fn mock_discovery(
	#[future] execution: ExecutionFixture,
	injection_context: InjectionContext,
) -> impl Future<Output = MockDiscovery> {
	let execution = Box::pin(execution);
	async move {
		let execution = execution.await;
		let state = DiscoveryReply {
			entry: execution
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
		};
		injection_context.set_singleton(state.clone());
		let server = test_server_guard(
			ServerRouter::new()
				.endpoint(discover)
				.with_di_context(Arc::new(injection_context)),
		)
		.await;
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
}
