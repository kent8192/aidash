//! Two native HTTP nodes with disposable databases and controllable network links.
use crate::native_database::{DatabaseFixture, DatabaseFuture, database};
#[path = "../../../execution/tests/support/browser_context.rs"]
pub(crate) mod browser_context;
#[path = "../../../execution/tests/support/federation.rs"]
pub(crate) mod peering;
#[path = "../../../execution/tests/support/tenant_roots.rs"]
pub(crate) mod tenant_roots;
use crate::settings::{process_settings, settings_for};
use aidash_server::{
	bootstrap,
	config::settings::ProjectSettings,
	federation::Federation,
	transactions::{Manifest, Status},
};
use chrono::{Duration, Utc};
use futures_util::{
	FutureExt,
	future::{BoxFuture, Shared},
};
use reinhardt::db::backends::DatabaseConnection as BackendConnection;
use reinhardt::db::orm::DatabaseConnectionLease;
use reinhardt::test::APIClient;
use reinhardt::test::fixtures::server::{TestServerGuard, test_server_guard};
use reinhardt::test::fixtures::{api_client_from_url, injection_context, singleton_scope};
use rstest::fixture;
use serde_json::{Value, json};
use std::{fs::File, future::Future, net::SocketAddr, process::Stdio, sync::Arc};
use tempfile::TempDir;
use tokio::{
	net::{TcpListener, TcpStream},
	process::{Child, Command},
	sync::watch,
	task::{JoinHandle, JoinSet},
};
use uuid::Uuid;

pub type Pair = (Node, Node, Manifest, Uuid, Uuid);

#[fixture]
pub fn pair(
	#[from(node)]
	#[with("a")]
	first: BoxFuture<'static, Node>,
	#[from(node)]
	#[with("b")]
	second: BoxFuture<'static, Node>,
) -> impl Future<Output = Option<Pair>> {
	Box::pin(async move {
		let mut a = first.await;
		let mut b = second.await;
		for node in [&mut a, &mut b] {
			// Each Deployment has one Tenant, with its default Node.
			tenant_roots::tenant_scope(&node.f.store, "acme").await;
			// The Node's client acts as the Tenant's Subject: a Deployment
			// Administrator holds no authority over Tenant content (#484).
			let token = tenant_roots::subject_credential(&node.f.store, "acme", "alice").await;
			node.client
				.set_header("Authorization", &format!("Bearer {token}"))
				.await
				.unwrap();
			node.address = tenant_roots::default_node(&node.f.store, "acme")
				.await
				.to_string();
		}
		connect(&mut a, &mut b).await;
		let mut workspaces = vec![];
		for (node, title) in [(&a, "A"), (&b, "B")] {
			let scope = tenant_roots::tenant_scope(&node.f.store, "acme").await;
			workspaces.push(
				node.f
					.store
					.create_workspace(&scope, "alice", title, "Atomic state")
					.await
					.unwrap()
					.id,
			);
		}
		let (wa, wb) = (workspaces[0], workspaces[1]);
		let manifest = serde_json::from_value(json!({
			"id":Uuid::new_v4(), "coordinator":a.address,
			"isolation":"serializable", "deadline":Utc::now()+Duration::minutes(5),
			"participants":[
				{"node_id":a.address,"mutations":[{"kind":"workspace_state",
					"workspace_id":wa,"expected_revision":0,"state":{"value":"new-a"}}]},
				{"node_id":b.address,"mutations":[{"kind":"workspace_state",
					"workspace_id":wb,"expected_revision":0,"state":{"value":"new-b"}}]}
			]
		}))
		.unwrap();
		Some((a, b, manifest, wa, wb))
	})
}

/// Each Deployment accepts and addresses the other's default Node, with
/// atomic transactions allowed. The Deployments reach each other on loopback.
pub async fn connect(a: &mut Node, b: &mut Node) {
	let (a_endpoint, b_endpoint) = (a.f.config.endpoint.clone(), b.f.config.endpoint.clone());
	a.f.store.keyless.permit_origin_of(&b_endpoint).unwrap();
	b.f.store.keyless.permit_origin_of(&a_endpoint).unwrap();
	a.settings
		.keyless_endpoints
		.permitted_private_origins
		.push(b_endpoint);
	b.settings
		.keyless_endpoints
		.permitted_private_origins
		.push(a_endpoint);
	for (local, remote) in [(&*a, &*b), (&*b, &*a)] {
		let owner = local.address.rsplit('/').next().unwrap();
		// Registrations and Peer Mappings are administered through a Tenant
		// Administrator Mapping (#484); the credential the mapping binds is
		// issued from the host.
		let administrator = local.administrator().await;
		let (status, body) = administrator
			.send(
				"POST",
				&peering::registration_path("acme", owner),
				peering::registration(
					&remote.address,
					Some((&remote.f.config.endpoint, &peering::key_id(&remote.f))),
				),
			)
			.await;
		assert_eq!(status, 200, "{body}");
		// The Tenant's subject `alice` on the other side maps to a local one.
		let issued = tenant_roots::issued_credential(&local.f.store, "acme", "alice").await;
		let input = serde_json::from_value(json!({
			"node":owner,"source_node":remote.address,"source_subject":"alice",
			"credential_id":issued["credential"]["id"],"enabled":true,"expected_revision":0,
		}))
		.unwrap();
		// The Tenant's HTTP route binds only service and agent credentials, so the
		// host writes this Mapping (#484).
		aidash_server::apps::identity::services::peer::write(&local.f, "acme", input)
			.await
			.unwrap();
	}
}

pub struct Node {
	// Drop network guards before releasing the database container.
	server: Option<Arc<TestServerGuard>>,
	link: Link,
	pub client: Arc<APIClient>,
	/// The DI context the running server resolves its services from.
	context: Arc<reinhardt::InjectionContext>,
	pub peer_client: Arc<APIClient>,
	pub f: Federation,
	settings: ProjectSettings,
	pub database: DatabaseFixture,
	/// The Tenant's default Node, once `pair` has written it.
	pub address: String,
	_key: Arc<peering::KeyFile>,
}

impl Node {
	async fn serve(
		settings: &ProjectSettings,
		connection: BackendConnection,
		link: &mut Link,
	) -> (
		Federation,
		TestServerGuard,
		Arc<reinhardt::InjectionContext>,
	) {
		// Act: rebuild DI and production routes after closing the participant pools.
		let context = injection_context(singleton_scope());
		let f = bootstrap::initialize(&context, settings, connection)
			.await
			.unwrap();
		let context = Arc::new(context);
		let router = aidash_server::routes()
			.with_di_context(context.clone())
			.into_server();
		let server = test_server_guard(router).await;
		link.route(Some(
			server.url.strip_prefix("http://").unwrap().parse().unwrap(),
		))
		.await;
		(f, server, context)
	}

	/// A Tenant Administrator acting through a browser Mapping of `alice`,
	/// which administers Node registrations and Peer Mappings (#484).
	pub async fn administrator(&self) -> browser_context::BrowserMapping {
		browser_context::BrowserMapping::on(
			&self.f,
			&self.context,
			self.peer_client.clone(),
			"acme",
			"alice",
		)
		.await
	}

	pub async fn stop(&mut self) {
		self.link.route(None).await;
		self.server.take();
	}

	pub async fn restart(&mut self) {
		self.stop().await;
		self.f.store.pool.close().await;
		self.f.store.control_pool.close().await;
		let connection =
			BackendConnection::connect_postgres_with_pool_size(&self.database.url, Some(12))
				.await
				.unwrap();
		self.database.lease = DatabaseConnectionLease::register(connection.clone()).unwrap();
		self.database.connection = connection.clone();
		let (f, server, context) = Self::serve(&self.settings, connection, &mut self.link).await;
		self.f = f;
		self.context = context;
		self.server = Some(Arc::new(server));
	}

	pub async fn get(&self, path: &str) -> (u16, Value) {
		let response = self.client.get(path).await.unwrap();
		(
			response.status_code(),
			response.json_value().unwrap_or(Value::Null),
		)
	}

	/// A POST of Tenant administration (Nodes, Node registrations) as the
	/// Tenant Administrator's browser Mapping (#484, ADR 0102).
	pub async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
		self.administrator()
			.await
			.send("POST", path, body.clone())
			.await
	}

	/// A GET with the shared Deployment Administrator bearer, for the routes
	/// that administer the Deployment and the refusals of everything else.
	pub async fn administrator_get(&self, path: &str) -> (u16, Value) {
		let response = self
			.peer_client
			.request(http::Method::GET, path)
			.header(
				"Authorization",
				format!("Bearer {}", self.f.config.api_token),
			)
			.send()
			.await
			.unwrap();
		(
			response.status_code(),
			response.json_value().unwrap_or(Value::Null),
		)
	}

	/// A POST with the shared Deployment Administrator bearer, such as an
	/// immediate Node purge (ADR 0083).
	pub async fn administrator_post(&self, path: &str, body: &Value) -> (u16, Value) {
		let response = self
			.peer_client
			.request(http::Method::POST, path)
			.header(
				"Authorization",
				format!("Bearer {}", self.f.config.api_token),
			)
			.json(body)
			.send()
			.await
			.unwrap();
		(
			response.status_code(),
			response.json_value().unwrap_or(Value::Null),
		)
	}

	/// The Sender for work the Tenant's default Node does as its subject.
	pub fn sender(&self) -> aidash_domain::federation::route::Sender {
		aidash_domain::federation::route::Sender {
			tenant: aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			node: None,
			subject: Some("alice".into()),
		}
	}

	/// Allows or refuses atomic transactions with `remote`'s Node, replacing the
	/// registration that holds `revision`. Returns the route's status.
	pub async fn set_trust(&self, remote: &Node, enabled: bool, revision: i64) -> u16 {
		let owner = self.address.rsplit('/').next().unwrap();
		let mut body = peering::registration(
			&remote.address,
			Some((&remote.f.config.endpoint, &peering::key_id(&remote.f))),
		);
		body["atomic_transactions"] = json!(enabled);
		body["expected_revision"] = json!(revision);
		self.administrator()
			.await
			.send("POST", &peering::registration_path("acme", owner), body)
			.await
			.0
	}

	/// Admits `manifest` as the host does: a transaction with no owning Subject.
	/// A Deployment Administrator has no HTTP route to submit one (#484); the
	/// Subject routes are covered by the scoped suites.
	pub async fn submit(&self, manifest: &Manifest) {
		let status = aidash_server::transactions::coordinator::submit(&self.f, manifest)
			.await
			.unwrap();
		assert_eq!(status.id, manifest.id);
	}

	pub async fn status(&self, id: Uuid) -> Status {
		aidash_server::transactions::coordinator::status(&self.f, id)
			.await
			.unwrap()
	}
}

/// A stable ephemeral address fronts the framework server. Partitioning closes
/// existing sockets as well as rejecting new connections, so pooled HTTP clients
/// cannot accidentally continue to talk to a stopped node.
struct Link {
	url: String,
	target: watch::Sender<Option<SocketAddr>>,
	applied: watch::Receiver<Option<SocketAddr>>,
	task: JoinHandle<()>,
}

impl Link {
	async fn route(&mut self, target: Option<SocketAddr>) {
		self.target.send_replace(target);
		// Wait for pooled connections to close before exposing the new link state.
		while *self.applied.borrow() != target {
			self.applied.changed().await.unwrap();
		}
	}

	fn from_listener(listener: TcpListener) -> Self {
		let url = format!("http://{}", listener.local_addr().unwrap());
		let (target, mut changes) = watch::channel::<Option<SocketAddr>>(None);
		let (applied_tx, applied) = watch::channel(None);
		let task = tokio::spawn(async move {
			let mut connections = JoinSet::new();
			loop {
				tokio::select! {
					change = changes.changed() => {
						if change.is_err() { break; }
						connections.shutdown().await;
						applied_tx.send_replace(*changes.borrow());
					},
					accepted = listener.accept() => {
						let (mut incoming, _) = accepted.unwrap();
						let target = *changes.borrow();
						if let Some(target) = target {
							connections.spawn(async move {
								if let Ok(mut outgoing) = TcpStream::connect(target).await {
									let _ = tokio::io::copy_bidirectional(&mut incoming, &mut outgoing).await;
								}
							});
						}
					},
					_ = connections.join_next(), if !connections.is_empty() => {},
				}
			}
		});
		Self {
			url,
			target,
			applied,
			task,
		}
	}
}

impl Drop for Link {
	fn drop(&mut self) {
		self.task.abort();
	}
}

/// Actual native worker process; settings, logs and credentials are fixture-owned.
pub struct WorkerProcess {
	process: Child,
	directory: TempDir,
}

impl WorkerProcess {
	pub async fn start(node: &Node, directory: TempDir) -> Self {
		std::fs::write(
			directory.path().join("base.toml"),
			process_settings(&node.settings),
		)
		.unwrap();
		let log = File::create(directory.path().join("worker.log")).unwrap();
		let process = Command::new(env!("CARGO_BIN_EXE_aidash"))
			.args(["worker"])
			.env_clear()
			.env("PATH", std::env::var_os("PATH").unwrap_or_default())
			.env("REINHARDT_SETTINGS_DIR", directory.path())
			.env("REINHARDT_ENV", "test")
			.env("RUST_BACKTRACE", "0")
			.current_dir(env!("CARGO_MANIFEST_DIR"))
			.stdin(Stdio::null())
			.stdout(log.try_clone().unwrap())
			.stderr(log)
			.kill_on_drop(true)
			.spawn()
			.unwrap();
		Self { process, directory }
	}

	pub fn assert_running(&mut self) {
		assert!(self.process.try_wait().unwrap().is_none(), "{}", self.log());
	}

	pub fn log(&self) -> String {
		std::fs::read_to_string(self.directory.path().join("worker.log")).unwrap()
	}

	pub async fn kill(&mut self) {
		self.process.kill().await.unwrap();
		let status = self.process.wait().await.unwrap();
		assert!(
			!status.success(),
			"worker must be killed, not exit gracefully"
		);
	}
}

type LinkFuture = Shared<BoxFuture<'static, Arc<tokio::sync::Mutex<Option<Link>>>>>;
#[fixture]
fn link() -> LinkFuture {
	// Raw TCP is required to close existing pooled connections during a partition.
	async {
		Arc::new(tokio::sync::Mutex::new(Some(Link::from_listener(
			TcpListener::bind("127.0.0.1:0").await.unwrap(),
		))))
	}
	.boxed()
	.shared()
}
#[derive(Clone)]
struct ProtocolRuntime {
	f: Federation,
	settings: ProjectSettings,
	database: DatabaseFixture,
	context: Arc<reinhardt::InjectionContext>,
	key: Arc<peering::KeyFile>,
}
type ProtocolRuntimeFuture = Shared<BoxFuture<'static, ProtocolRuntime>>;
#[fixture]
fn protocol_runtime(
	#[default("a")] suffix: &str,
	database: DatabaseFuture,
	link: LinkFuture,
	injection_context: reinhardt::InjectionContext,
) -> ProtocolRuntimeFuture {
	let suffix = suffix.to_owned();
	async move {
		let database = database.await;
		let link = link.await;
		let guard = link.lock().await;
		let mut settings = settings_for(&database.url);
		settings.deployment.id = format!("aidash://atomic-{suffix}");
		settings.deployment.endpoint = guard.as_ref().unwrap().url.clone();
		settings.deployment.api_token = "atomic-deployment-administrator-fixture-token".into();
		settings.deployment.nats_url = "nats://127.0.0.1:0".into();
		// The Deployment signs with a key file, as `[deployment.key] file` names it.
		let key = peering::KeyFile::generate();
		peering::configure_key(&mut settings, &key);
		let f = bootstrap::initialize(&injection_context, &settings, database.connection.clone())
			.await
			.unwrap();
		ProtocolRuntime {
			f,
			settings,
			database,
			context: Arc::new(injection_context),
			key,
		}
	}
	.boxed()
	.shared()
}
type ProtocolServerFuture = Shared<BoxFuture<'static, Arc<TestServerGuard>>>;
#[fixture]
fn protocol_server(protocol_runtime: ProtocolRuntimeFuture) -> ProtocolServerFuture {
	async move {
		let runtime = protocol_runtime.await;
		Arc::new(
			test_server_guard(
				aidash_server::routes()
					.with_di_context(runtime.context)
					.into_server(),
			)
			.await,
		)
	}
	.boxed()
	.shared()
}
type ProtocolClientFuture = Shared<BoxFuture<'static, Arc<APIClient>>>;
#[fixture]
fn protocol_client(
	protocol_runtime: ProtocolRuntimeFuture,
	#[default(true)] deployment_administrator: bool,
) -> ProtocolClientFuture {
	async move {
		let runtime = protocol_runtime.await;
		let client = api_client_from_url(&runtime.settings.deployment.endpoint);
		if deployment_administrator {
			client
				.set_header(
					"Authorization",
					&format!("Bearer {}", runtime.f.config.api_token),
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
fn node(
	#[default("a")] _suffix: &str,
	#[from(database)] _database: DatabaseFuture,
	link: LinkFuture,
	#[from(protocol_runtime)]
	#[with(_suffix,_database.clone(),link.clone())]
	runtime: ProtocolRuntimeFuture,
	#[from(protocol_server)]
	#[with(runtime.clone())]
	server: ProtocolServerFuture,
	#[from(protocol_client)]
	#[with(runtime.clone())]
	client: ProtocolClientFuture,
	#[from(protocol_client)]
	#[with(runtime.clone(),false)]
	peer_client: ProtocolClientFuture,
) -> BoxFuture<'static, Node> {
	async move {
		let runtime = runtime.await;
		let server = server.await;
		let mut link = link.await.lock().await.take().unwrap();
		link.route(Some(
			server.url.strip_prefix("http://").unwrap().parse().unwrap(),
		))
		.await;
		Node {
			server: Some(server),
			link,
			client: client.await,
			context: runtime.context.clone(),
			peer_client: peer_client.await,
			f: runtime.f,
			settings: runtime.settings,
			database: runtime.database,
			address: String::new(),
			_key: runtime.key,
		}
	}
	.boxed()
}
