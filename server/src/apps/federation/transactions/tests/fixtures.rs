//! Two native HTTP nodes with disposable databases and controllable network links.
use crate::native_database::{DatabaseFixture, database};
use crate::settings::{process_settings, settings_for};
use aidash_server::{
	bootstrap,
	config::settings::ProjectSettings,
	federation::{Federation, Peer},
	transactions::{Manifest, Status},
};
use chrono::{Duration, Utc};
use reinhardt::db::backends::DatabaseConnection as BackendConnection;
use reinhardt::db::orm::DatabaseConnectionLease;
use reinhardt::test::APIClient;
use reinhardt::test::fixtures::server::{TestServerGuard, test_server_guard};
use reinhardt::test::fixtures::{
	api_client_from_url, injection_context, singleton_scope, temp_dir,
};
use rstest::fixture;
use serde_json::{Value, json};
use std::{
	fs::File,
	future::Future,
	net::SocketAddr,
	process::Stdio,
	sync::{Arc, LazyLock},
	time::Duration as Wait,
};
use tempfile::TempDir;
use tokio::{
	net::{TcpListener, TcpStream},
	process::{Child, Command},
	sync::{Semaphore, watch},
	task::{JoinHandle, JoinSet},
};
use uuid::Uuid;

pub const PEER_SECRET: &str = "atomic-protocol-fixture-0123456789-ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const PEER_ENV: &str = "AIDASH_SECRET_ATOMIC_PROTOCOL_FIXTURE";

pub type Pair = (Node, Node, Manifest, Uuid, Uuid);

/// Scope environment-backed peer credentials to a fresh test process. No test
/// changes its parent process environment while HTTP/runtime threads are alive.
async fn isolated_process() -> bool {
	let name = std::thread::current()
		.name()
		.expect("named libtest thread")
		.to_owned();
	if std::env::var("AIDASH_PROTOCOL_TEST_CHILD").as_deref() == Ok(name.as_str()) {
		assert_eq!(std::env::var(PEER_ENV).unwrap(), PEER_SECRET);
		return true;
	}
	static CAPACITY: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(2));
	let _capacity = CAPACITY.acquire().await.unwrap();
	let child = Command::new(std::env::current_exe().unwrap())
		.args(["--exact", &name, "--nocapture"])
		.env("AIDASH_PROTOCOL_TEST_CHILD", &name)
		.env(PEER_ENV, PEER_SECRET)
		.env("RUST_BACKTRACE", "0")
		.kill_on_drop(true)
		.output();
	let output = tokio::time::timeout(Wait::from_secs(180), child)
		.await
		.expect("isolated protocol test deadline")
		.unwrap();
	assert!(
		output.status.success(),
		"{name}: {}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	assert!(
		String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed; 0 ignored"),
		"isolated process must execute exactly the selected test: {}",
		String::from_utf8_lossy(&output.stdout)
	);
	false
}

#[fixture]
pub fn pair(
	#[future]
	#[from(database)]
	first: DatabaseFixture,
	#[future]
	#[from(database)]
	second: DatabaseFixture,
) -> impl Future<Output = Option<Pair>> {
	// Keep the framework bootstrap futures off the default libtest thread stack.
	let first = Box::pin(first);
	let second = Box::pin(second);
	Box::pin(async move {
		if !isolated_process().await {
			return None;
		}
		let a = Node::new(first.await, "a").await;
		let b = Node::new(second.await, "b").await;
		for (local, remote) in [(&a, &b), (&b, &a)] {
			local
				.f
				.register_peer(Peer {
					node_id: remote.f.config.node_id.clone(),
					endpoint: remote.f.config.endpoint.clone(),
					credential_env: PEER_ENV.into(),
					protocol_version: "0.2".into(),
					enabled: true,
				})
				.await
				.unwrap();
			assert_eq!(
				local
					.post(
						"/api/transactions/trust",
						&json!({
							"node_id": remote.f.config.node_id, "enabled": true
						})
					)
					.await
					.0,
				200
			);
		}
		let wa = a
			.post(
				"/api/workspaces",
				&json!({"title":"A", "goal":"Atomic state"}),
			)
			.await;
		let wb = b
			.post(
				"/api/workspaces",
				&json!({"title":"B", "goal":"Atomic state"}),
			)
			.await;
		assert_eq!(wa.0, 200, "{}", wa.1);
		assert_eq!(wb.0, 200, "{}", wb.1);
		let wa: Uuid = serde_json::from_value(wa.1["id"].clone()).unwrap();
		let wb: Uuid = serde_json::from_value(wb.1["id"].clone()).unwrap();
		let manifest = serde_json::from_value(json!({
			"id":Uuid::new_v4(), "coordinator":a.f.config.node_id,
			"isolation":"serializable", "deadline":Utc::now()+Duration::minutes(5),
			"participants":[
				{"node_id":a.f.config.node_id,"mutations":[{"kind":"workspace_state",
					"workspace_id":wa,"expected_revision":0,"state":{"value":"new-a"}}]},
				{"node_id":b.f.config.node_id,"mutations":[{"kind":"workspace_state",
					"workspace_id":wb,"expected_revision":0,"state":{"value":"new-b"}}]}
			]
		}))
		.unwrap();
		Some((a, b, manifest, wa, wb))
	})
}

pub struct Node {
	// Drop network guards before releasing the database container.
	server: Option<TestServerGuard>,
	link: Link,
	pub client: APIClient,
	pub f: Federation,
	settings: ProjectSettings,
	pub database: DatabaseFixture,
}

impl Node {
	async fn new(database: DatabaseFixture, suffix: &str) -> Self {
		let mut link = Link::new().await;
		let mut settings = settings_for(&database.url);
		settings.node.node_id = format!("aidash://atomic-{suffix}");
		settings.node.endpoint = link.url.clone();
		settings.node.api_token = "atomic-operator-fixture-token".into();
		settings.node.nats_url = "nats://127.0.0.1:0".into();
		let (f, server) = Self::serve(&settings, database.connection.clone(), &mut link).await;
		let client = api_client_from_url(&link.url);
		client
			.set_header("Authorization", &format!("Bearer {}", f.config.api_token))
			.await
			.unwrap();
		Self {
			f,
			client,
			database,
			settings,
			server: Some(server),
			link,
		}
	}

	async fn serve(
		settings: &ProjectSettings,
		connection: BackendConnection,
		link: &mut Link,
	) -> (Federation, TestServerGuard) {
		let context = injection_context(singleton_scope());
		let f = bootstrap::initialize(&context, settings, connection)
			.await
			.unwrap();
		let router = aidash_server::routes()
			.with_di_context(Arc::new(context))
			.into_server();
		let server = test_server_guard(router).await;
		link.route(Some(
			server.url.strip_prefix("http://").unwrap().parse().unwrap(),
		))
		.await;
		(f, server)
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
		let (f, server) = Self::serve(&self.settings, connection, &mut self.link).await;
		self.f = f;
		self.server = Some(server);
	}

	pub async fn get(&self, path: &str) -> (u16, Value) {
		let response = self.client.get(path).await.unwrap();
		(
			response.status_code(),
			response.json_value().unwrap_or(Value::Null),
		)
	}

	pub async fn post(&self, path: &str, body: &impl serde::Serialize) -> (u16, Value) {
		let response = self.client.post(path, body, "json").await.unwrap();
		(
			response.status_code(),
			response.json_value().unwrap_or(Value::Null),
		)
	}

	pub async fn submit(&self, manifest: &Manifest) {
		let (status, body) = self.post("/api/transactions", manifest).await;
		assert_eq!(status, 202, "{body}");
		assert_eq!(body["id"], json!(manifest.id));
	}

	pub async fn status(&self, id: Uuid) -> Status {
		let (status, body) = self.get(&format!("/api/transactions/{id}")).await;
		assert_eq!(status, 200, "{body}");
		serde_json::from_value(body["transaction"].clone()).unwrap()
	}

	pub async fn peer_client(&self, caller: &Node) -> APIClient {
		let client = api_client_from_url(&self.link.url);
		for (key, value) in [
			("authorization", format!("Bearer {PEER_SECRET}")),
			("x-aidash-node", caller.f.config.node_id.clone()),
			("x-aidash-protocol", "0.2".into()),
		] {
			client.set_header(key, &value).await.unwrap();
		}
		client
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

	async fn new() -> Self {
		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
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
	pub async fn start(node: &Node) -> Self {
		let directory = temp_dir();
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
			.env(PEER_ENV, PEER_SECRET)
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
