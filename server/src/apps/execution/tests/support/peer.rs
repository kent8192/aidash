//! Peer origins are declared before production runtime and route composition.
#![allow(dead_code)] // Shared integration binaries consume different peer scenarios.
use super::upstream_fixtures as transport;
use super::{
	ApplicationFuture, RouterTransform, RuntimeFixture, RuntimeFuture, TestApplication,
	native_application, runtime,
};
use futures_util::{
	FutureExt,
	future::{BoxFuture, Shared},
};
use rstest::fixture;
use std::sync::Arc;
pub use transport::FixedServerGuard;
use transport::{ListenerFuture, RouterFuture, fixed_listener, fixed_upstream};

#[derive(Clone)]
pub struct PeerFixture {
	pub runtime: RuntimeFixture,
	pub application: TestApplication,
	pub client: Arc<reinhardt::test::APIClient>,
	pub server: Arc<FixedServerGuard>,
}
pub type PeerFuture = Shared<BoxFuture<'static, PeerFixture>>;
#[derive(Clone)]
pub struct PeerApplication {
	runtime: RuntimeFixture,
	application: TestApplication,
	listener: ListenerFuture,
}
pub type PeerApplicationFuture = Shared<BoxFuture<'static, PeerApplication>>;

#[fixture]
fn peer_runtime(
	#[default("aidash://execution-test")] node_id: &str,
	fixed_listener: ListenerFuture,
	runtime: RuntimeFuture,
) -> RuntimeFuture {
	let node_id = node_id.to_owned();
	async move {
		let mut owner = runtime.await;
		let f = &mut owner.federation;
		f.config.node_id = node_id;
		f.store.node_id = f.config.node_id.clone();
		f.config.endpoint = format!("http://{}", fixed_listener.await.local_addr().unwrap());
		f.registry =
			aidash_server::registry::Registry::new(f.store.pool.clone(), &f.config.node_id)
				.unwrap();
		owner
	}
	.boxed()
	.shared()
}
#[fixture]
fn peer_application(
	#[default("aidash://execution-test")] _node_id: &str,
	#[default(Arc::new(|router| router))] _transform: RouterTransform,
	#[from(runtime)] _base: RuntimeFuture,
	#[default(aidash_server::sse::Service::new(Default::default()))]
	_streams: aidash_server::sse::Service,
	#[from(fixed_listener)] _listener: ListenerFuture,
	#[from(peer_runtime)]
	#[with(_node_id,_listener.clone(),_base.clone())]
	_runtime: RuntimeFuture,
	#[from(native_application)]
	#[with(Default::default(), _streams.clone(), _transform.clone(), _runtime.clone())]
	_application: ApplicationFuture,
) -> PeerApplicationFuture {
	async move {
		PeerApplication {
			runtime: _runtime.await,
			application: _application.await.application,
			listener: _listener,
		}
	}
	.boxed()
	.shared()
}
#[fixture]
fn peer_router(peer_application: PeerApplicationFuture) -> RouterFuture {
	async move { peer_application.await.application.native_router() }
		.boxed()
		.shared()
}
#[fixture]
fn peer_listener(peer_application: PeerApplicationFuture) -> ListenerFuture {
	async move { peer_application.await.listener.await }
		.boxed()
		.shared()
}
pub type PeerServerFuture = Shared<BoxFuture<'static, Arc<FixedServerGuard>>>;
#[fixture]
fn peer_server(
	#[from(peer_application)] _application: PeerApplicationFuture,
	#[from(peer_router)]
	#[with(_application.clone())]
	_router: RouterFuture,
	#[from(peer_listener)]
	#[with(_application.clone())]
	_listener: ListenerFuture,
	#[from(fixed_upstream)]
	#[with(None,_listener.clone(),_router.clone())]
	server: BoxFuture<'static, FixedServerGuard>,
) -> PeerServerFuture {
	let server = Box::pin(server);
	async move { Arc::new(server.await) }.boxed().shared()
}
type PeerTransportFuture =
	Shared<BoxFuture<'static, (Arc<FixedServerGuard>, Arc<reinhardt::test::APIClient>)>>;
/// Serve the peer and declare its client as one natural transport dependency.
#[fixture]
fn peer_transport(
	#[from(peer_application)] _application: PeerApplicationFuture,
	#[from(peer_server)]
	#[with(_application.clone())]
	server: PeerServerFuture,
) -> PeerTransportFuture {
	async move {
		let server = server.await;
		let client = Arc::new(
			reinhardt::test::APIClient::builder()
				.base_url(&server.url)
				.timeout(std::time::Duration::from_secs(5))
				.build(),
		);
		(server, client)
	}
	.boxed()
	.shared()
}
#[fixture]
pub fn native_peer(
	#[default("aidash://execution-test")] _node_id: &str,
	#[default(Arc::new(|router| router))] _transform: RouterTransform,
	#[from(runtime)] _base: RuntimeFuture,
	#[default(aidash_server::sse::Service::new(Default::default()))]
	_streams: aidash_server::sse::Service,
	// Callers that hand the socket to child processes own its reservation.
	#[from(fixed_listener)] _listener: ListenerFuture,
	#[from(peer_application)]
	#[with(_node_id,_transform.clone(),_base.clone(),_streams.clone(),_listener.clone())]
	_application: PeerApplicationFuture,
	#[from(peer_transport)]
	#[with(_application.clone())]
	transport: PeerTransportFuture,
) -> PeerFuture {
	async move {
		let application = _application.await;
		let (server, client) = transport.await;
		PeerFixture {
			runtime: application.runtime,
			application: application.application,
			server,
			client,
		}
	}
	.boxed()
	.shared()
}
