use super::*;
use reinhardt::test::testcontainers::{
	GenericImage, ImageExt, core::IntoContainerPort, runners::AsyncRunner,
};
use std::{
	path::{Path, PathBuf},
	process::{Child, Command},
};
use tokio::sync::{mpsc, watch};
#[path = "benchmark.rs"]
mod benchmark;

fn directory(label: &str, schema: &str) -> PathBuf {
	let root = std::env::var_os("AIDASH_SSE_EVIDENCE_DIR")
		.map(PathBuf::from)
		.unwrap_or_else(|| std::env::temp_dir().join("aidash-sse-evidence"));
	let dir = root.join(format!("{label}-{schema}"));
	std::fs::create_dir_all(&dir).unwrap();
	dir
}
fn free_port() -> u16 {
	// Child binaries bind their own HTTP and metrics listeners; reserve a released port for each lifecycle Act.
	std::net::TcpListener::bind("127.0.0.1:0")
		.unwrap()
		.local_addr()
		.unwrap()
		.port()
}
fn candidate_binary() -> PathBuf {
	std::env::var_os("AIDASH_SSE_CANDIDATE_BINARY")
		.map(PathBuf::from)
		.unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_aidash")))
}

struct Process {
	child: Child,
	endpoint: String,
	metrics: String,
	log: PathBuf,
}
impl Process {
	fn start(
		fixture: &Fixture,
		dir: &Path,
		ordinal: usize,
		interval_ms: u64,
		binary: &Path,
	) -> Self {
		let port = free_port();
		let metrics_port = free_port();
		let endpoint = format!("http://127.0.0.1:{port}");
		let metrics = format!("http://127.0.0.1:{metrics_port}/metrics");
		let mut database = reqwest::Url::parse(&fixture.url).unwrap();
		database.query_pairs_mut().append_pair(
			"options",
			&format!("-c application_name=sse-process-{ordinal}"),
		);
		let log = dir.join(format!("server-{ordinal}.log"));
		let file = std::fs::File::create(&log).unwrap();
		let mut native = fixture.f.clone();
		native.config.endpoint = endpoint.clone();
		let child = Command::new(binary)
			.args(
				if binary
					.file_name()
					.is_some_and(|name| name == "baseline-aidash")
				{
					vec!["server".into()]
				} else {
					common::native_process_args(&native, "server")
				},
			)
			.envs(common::native_process_environment(
				&native,
				database.as_str(),
				dir,
				0,
			))
			.env("DATABASE_URL", database.as_str())
			.env("AIDASH_NODE_ID", &fixture.f.config.node_id)
			.env("AIDASH_ENDPOINT", &endpoint)
			.env("AIDASH_LISTEN", format!("127.0.0.1:{port}"))
			.env("AIDASH_METRICS_LISTEN", format!("127.0.0.1:{metrics_port}"))
			.env("AIDASH_API_TOKEN", &fixture.f.config.api_token)
			.env("NATS_URL", &fixture.f.config.nats_url)
			.env("AIDASH_ACTIVATION_NAMESPACE", &fixture.schema)
			.env("AIDASH_ACTIVATION_BOOTSTRAP", "true")
			.env("AIDASH_SSE_RECONCILE_INTERVAL_MS", interval_ms.to_string())
			.env("AIDASH_SSE_BACKPRESSURE_TIMEOUT_SECONDS", "30")
			.env("AIDASH_ENV", "test")
			.env("RUST_LOG", "aidash=info")
			.stdout(file.try_clone().unwrap())
			.stderr(file)
			.spawn()
			.unwrap();
		Self {
			child,
			endpoint,
			metrics,
			log,
		}
	}
	async fn ready(&mut self, fixture: &Fixture, subscriber: bool) {
		let began = Instant::now();
		let client = fixture.f.client.clone();
		loop {
			let log = std::fs::read_to_string(&self.log).unwrap();
			assert!(
				self.child.try_wait().unwrap().is_none(),
				"process exited: {log}"
			);
			if (!subscriber || log.contains("SSE notification subscription ready"))
				&& client
					.get(format!("{}/api/events?after={}", self.endpoint, i64::MAX))
					.bearer_auth(&fixture.token)
					.send()
					.await
					.is_ok_and(|r| r.status().is_success())
				&& client
					.get(&self.metrics)
					.send()
					.await
					.is_ok_and(|r| r.status().is_success())
			{
				break;
			}
			assert!(
				began.elapsed() < Duration::from_secs(30),
				"process startup deadline: {log}"
			);
			tokio::time::sleep(Duration::from_millis(50)).await;
		}
	}
	async fn metrics(&self) -> String {
		reqwest::get(&self.metrics)
			.await
			.unwrap()
			.error_for_status()
			.unwrap()
			.text()
			.await
			.unwrap()
	}
	async fn stop(&mut self) {
		assert!(
			Command::new("kill")
				.args(["-TERM", &self.child.id().to_string()])
				.status()
				.unwrap()
				.success()
		);
		let began = Instant::now();
		while self.child.try_wait().unwrap().is_none() {
			assert!(
				began.elapsed() < Duration::from_secs(22),
				"SIGTERM drain deadline"
			);
			tokio::time::sleep(Duration::from_millis(20)).await;
		}
	}
}
impl Drop for Process {
	fn drop(&mut self) {
		let _ = self.child.kill();
		let _ = self.child.wait();
	}
}

fn metric(text: &str, name: &str) -> f64 {
	text.lines()
		.filter(|line| {
			line.starts_with(name)
				&& (line.as_bytes().get(name.len()) == Some(&b' ')
					|| line.as_bytes().get(name.len()) == Some(&b'{'))
		})
		.map(|line| line.rsplit_once(' ').unwrap().1.parse::<f64>().unwrap())
		.sum()
}
fn cause(text: &str, reason: &str) -> f64 {
	text.lines()
		.filter(|line| {
			line.starts_with("aidash_sse_query_causes_total{")
				&& line.contains(&format!("reason=\"{reason}\""))
		})
		.map(|line| line.rsplit_once(' ').unwrap().1.parse::<f64>().unwrap())
		.sum()
}

struct Received {
	client: usize,
	id: i64,
	data: Value,
	at: Instant,
}
struct RemoteStream {
	task: tokio::task::JoinHandle<()>,
}
impl Drop for RemoteStream {
	fn drop(&mut self) {
		self.task.abort();
	}
}
async fn observe(
	process: &Process,
	fixture: &Fixture,
	client_id: usize,
	workspace: Option<Uuid>,
	after: i64,
	output: mpsc::UnboundedSender<Received>,
) -> RemoteStream {
	let mut path = format!("{}/api/events/stream?after={after}", process.endpoint);
	if let Some(workspace) = workspace {
		path.push_str(&format!("&workspace_id={workspace}"));
	}
	let response = fixture
		.f
		.client
		.clone()
		.get(path)
		.bearer_auth(&fixture.token)
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 200);
	let task = tokio::spawn(async move {
		let mut bytes = response.bytes_stream();
		let mut buffer = Vec::new();
		while let Some(chunk) = bytes.next().await {
			let Ok(chunk) = chunk else {
				return;
			};
			buffer.extend_from_slice(&chunk);
			while let Some(end) = buffer.windows(2).position(|w| w == b"\n\n") {
				let text = String::from_utf8(buffer.drain(..end + 2).collect()).unwrap();
				if let Some(id) = text.lines().find_map(|line| line.strip_prefix("id: ")) {
					let data = text
						.lines()
						.find_map(|line| line.strip_prefix("data: "))
						.unwrap();
					let _ = output.send(Received {
						client: client_id,
						id: id.parse().unwrap(),
						data: serde_json::from_str(data).unwrap(),
						at: Instant::now(),
					});
				}
			}
		}
	});
	RemoteStream { task }
}
async fn receive(
	output: &mut mpsc::UnboundedReceiver<Received>,
	count: usize,
	seconds: u64,
) -> Vec<Received> {
	tokio::time::timeout(Duration::from_secs(seconds), async {
		let mut result = Vec::new();
		while result.len() < count {
			result.push(output.recv().await.expect("observer closed"));
		}
		result
	})
	.await
	.expect("complete fan-out deadline")
}

// A TCP fault boundary affects every real broker connection, including existing
// ones. It does not replace NATS, generate notifications or touch event data.
struct Proxy {
	endpoint: String,
	online: watch::Sender<bool>,
	task: tokio::task::JoinHandle<()>,
}
impl Proxy {
	async fn new(destination: &str, available: bool) -> Self {
		let destination = reqwest::Url::parse(destination).unwrap();
		let target = format!(
			"{}:{}",
			destination.host_str().unwrap(),
			destination.port().unwrap()
		);
		// A byte-level NATS outage proxy must control active TCP bridges; HTTP fixtures cannot replace this transport.
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let endpoint = format!("nats://{}", listener.local_addr().unwrap());
		let (online, changes) = watch::channel(available);
		let task = tokio::spawn(async move {
			let mut connections = tokio::task::JoinSet::new();
			loop {
				tokio::select! {
					result = listener.accept() => {
						let (mut incoming, _) = result.unwrap();
						if !*changes.borrow() { continue; }
						let target = target.clone();
						let mut changes = changes.clone();
						changes.borrow_and_update();
						connections.spawn(async move {
							let Ok(mut broker) = tokio::net::TcpStream::connect(target).await else { return; };
							tokio::select! {
								_ = changes.changed() => {},
								_ = tokio::io::copy_bidirectional(&mut incoming, &mut broker) => {},
							}
						});
					}
					_ = connections.join_next(), if !connections.is_empty() => {},
				}
			}
		});
		Self {
			endpoint,
			online,
			task,
		}
	}
}
impl Drop for Proxy {
	fn drop(&mut self) {
		self.task.abort();
	}
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn replicas_recover_broker_outages_and_replay_after_shutdown(
	#[from(sse_fixture)]
	#[with(Duration::from_secs(60), Duration::from_secs(30), 128)]
	root: SseFuture,
	#[future(awt)] outage_transport: OutageTransport,

	#[from(evidence_directory)]
	#[with("replicas".into(),root.clone())]
	dir_future: DirectoryFuture,
) {
	let mut fixture = Arc::try_unwrap(root.await).ok().unwrap();
	let OutageTransport {
		broker,
		proxy,
		broker_url,
	} = outage_transport;
	let dir = dir_future.await;
	// This test restarts its own broker, so parallel cases retain their services.

	fixture.f.config.nats_url = proxy.endpoint.clone();
	let binary = candidate_binary();
	let mut processes = Vec::new();
	// Act: launch each replica after applying its broker availability or role configuration.
	for n in 0..3 {
		let mut process = Process::start(&fixture, &dir, n, 5000, &binary);
		process.ready(&fixture, false).await; // Broker absence cannot delay HTTP startup.
		processes.push(process);
	}
	let (send, mut received) = mpsc::unbounded_channel();
	let mut streams = Vec::new();
	let ws = fixture.workspaces[0];
	for (n, process) in processes.iter().enumerate() {
		streams.push(observe(process, &fixture, n, Some(ws), -1, send.clone()).await);
	}
	let lost = fixture.emit(ws, 1).await;
	for receipt in receive(&mut received, 3, 7).await {
		assert_eq!(receipt.id, lost.sequence);
	}
	for p in &processes {
		assert_eq!(
			metric(&p.metrics().await, "aidash_sse_subscriber_ready"),
			0.0
		);
	}
	proxy.online.send_replace(true);
	for p in &mut processes {
		p.ready(&fixture, true).await;
	}
	// Quiesce initial Outbox drain/reconnect before asserting notification-only causality.
	tokio::time::sleep(Duration::from_secs(1)).await;
	let before = futures_util::future::join_all(processes.iter().map(Process::metrics)).await;
	let client = fixture.f.client.clone();
	let began = Instant::now();
	let response = client
		.patch(format!("{}/api/workspaces/{ws}", processes[0].endpoint))
		.bearer_auth(&fixture.token)
		.json(&json!({"revision":0,"state":{"number":2}}))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
	let observations = receive(&mut received, 3, 2).await;
	assert!(began.elapsed() < Duration::from_secs(2));
	let id = observations[0].id;
	for observation in &observations {
		assert_eq!(observation.id, id);
		assert_eq!(observation.data["data"]["state"]["number"], 2);
	}
	let after = futures_util::future::join_all(processes.iter().map(Process::metrics)).await;
	// Act: launch each replica after applying its broker availability or role configuration.
	for n in 0..3 {
		assert!(cause(&after[n], "notification") > cause(&before[n], "notification"));
	}
	// Production's independent execution consumer must also acknowledge the event.
	let bus = aidash_server::bus::EventBus::connect(&broker_url, &fixture.f.config.node_id)
		.await
		.unwrap();
	let stream = bus.context.get_stream(&bus.stream_name).await.unwrap();
	let mut consumer = stream
		.get_consumer::<async_nats::jetstream::consumer::pull::Config>("execution")
		.await
		.unwrap();
	assert!(consumer.info().await.unwrap().ack_floor.consumer_sequence > 0);
	proxy.online.send_replace(false);
	tokio::time::sleep(Duration::from_millis(500)).await;
	bus.context.delete_stream(&bus.stream_name).await.unwrap();
	broker.stop().await.unwrap();
	broker.start().await.unwrap();
	let missed = fixture.emit(ws, 3).await;
	for receipt in receive(&mut received, 3, 7).await {
		assert_eq!(receipt.id, missed.sequence);
	}
	// Reconnection must reconcile immediately, even without publishing a new hint.
	let recovery = fixture.emit(ws, 4).await;
	proxy.online.send_replace(true);
	for receipt in receive(&mut received, 3, 7).await {
		assert_eq!(receipt.id, recovery.sequence);
	}
	processes[2].stop().await;
	drop(streams.remove(2));
	let replay = fixture.emit(ws, 5).await;
	streams.push(
		observe(
			&processes[1],
			&fixture,
			2,
			Some(ws),
			recovery.sequence,
			send.clone(),
		)
		.await,
	);
	for receipt in receive(&mut received, 3, 7).await {
		assert_eq!(receipt.id, replay.sequence);
	}
	processes[0].child.kill().unwrap();
	processes[0].child.wait().unwrap();
	drop(streams.remove(0));
	let crash_replay = fixture.emit(ws, 6).await;
	streams.push(observe(&processes[1], &fixture, 0, Some(ws), replay.sequence, send).await);
	for receipt in receive(&mut received, 3, 7).await {
		assert_eq!(receipt.id, crash_replay.sequence);
	}
	std::fs::write(dir.join("assertions.json"), serde_json::to_vec_pretty(&json!({"replicas":3,"startup_without_broker":true,"canonical_fallback":true,"execution_ack":true,"broker_restart_with_lost_stream":true,"sigterm_replay":true,"crash_replay":true})).unwrap()).unwrap();
	drop(streams);
	processes[1].stop().await;
	drop(processes);
	fixture.finish().await;
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_reference_notification_replays_canonical_bytes(
	#[from(sse_fixture)]
	#[with(Duration::from_secs(60), Duration::from_secs(30), 128)]
	root: SseFuture,

	#[from(evidence_directory)]
	#[with("oversized".into(),root.clone())]
	dir_future: DirectoryFuture,
	#[future(awt)]
	#[from(initial_process)]
	#[with(root.clone(),dir_future.clone())]
	initial: Process,
) {
	let fixture = root.await;
	let dir = dir_future.await;
	let mut process = initial;
	let (send, mut received) = mpsc::unbounded_channel();
	let ws = fixture.workspaces[0];
	let stream = observe(&process, &fixture, 0, Some(ws), -1, send).await;
	let warmup = fixture.emit(ws, 1).await;
	assert_eq!(receive(&mut received, 1, 3).await[0].id, warmup.sequence);
	let nats = async_nats::connect(&fixture.f.config.nats_url)
		.await
		.unwrap();
	let mut hints = nats
		.subscribe(format!(
			"aidash.{}.events",
			fixture.f.config.node_id.trim_start_matches("aidash://")
		))
		.await
		.unwrap();
	nats.flush().await.unwrap();
	let before = process.metrics().await;
	let memory = || {
		String::from_utf8(
			Command::new("ps")
				.args(["-p", &process.child.id().to_string(), "-o", "rss="])
				.output()
				.unwrap()
				.stdout,
		)
		.unwrap()
		.trim()
		.parse::<u64>()
		.unwrap()
	};
	let rss_before = memory();
	let body_bytes = nats.server_info().max_payload + 1024;
	let event = fixture
		.f
		.store
		.emit(
			Some(ws),
			"workspace.updated",
			json!({"id":ws,"text":"x".repeat(body_bytes)}),
		)
		.await
		.unwrap();
	let envelope = tokio::time::timeout(Duration::from_secs(5), async {
		loop {
			let hint: Value = serde_json::from_slice(&hints.next().await.unwrap().payload).unwrap();
			if hint["id"] == event.id.to_string() {
				break hint;
			}
		}
	})
	.await
	.unwrap();
	assert!(envelope.get("data").is_none());
	assert!(envelope["dataref"].is_string());
	let frames = receive(&mut received, 1, 3).await;
	assert_eq!(frames[0].id, event.sequence);
	assert_eq!(frames[0].data, event.cloud_event());
	let after = process.metrics().await;
	assert_eq!(cause(&before, "fallback"), cause(&after, "fallback"));
	assert!(cause(&after, "notification") > cause(&before, "notification"));
	std::fs::write(dir.join("assertions.json"), serde_json::to_vec_pretty(&json!({"body_bytes":body_bytes,"nats_max_payload":nats.server_info().max_payload,"reference_notification":true,"canonical_payload_equal":true,"rss_before_kib":rss_before,"rss_after_kib":memory()})).unwrap()).unwrap();
	drop(stream);
	process.stop().await;
	drop(process);
	fixture.finish().await;
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn writer_a_execution_b_and_sse_c_have_independent_delivery(
	#[from(sse_fixture)]
	#[with(Duration::from_secs(60), Duration::from_secs(30), 128)]
	root: SseFuture,
	#[future(awt)] restricted_broker: reinhardt::test::testcontainers::ContainerAsync<GenericImage>,

	#[from(evidence_directory)]
	#[with("independent-consumer".into(),root.clone())]
	dir_future: DirectoryFuture,
) {
	let mut fixture = Arc::try_unwrap(root.await).ok().unwrap();
	let broker = restricted_broker;
	let dir = dir_future.await;
	// A/C may publish and subscribe to UI hints, but only B may pull from the
	// shared execution consumer. This makes process ownership deterministic.

	let host = broker.get_host().await.unwrap();
	let port = broker.get_host_port_ipv4(4222.tcp()).await.unwrap();
	let mut processes = Vec::new();
	// Act: launch each replica after applying its broker availability or role configuration.
	for n in 0..3 {
		let user = if n == 1 { "executor" } else { "observer" };
		fixture.f.config.nats_url = format!("nats://{user}:fixture@{host}:{port}");
		let mut process = Process::start(&fixture, &dir, n, 60000, &candidate_binary());
		process.ready(&fixture, true).await;
		processes.push(process);
	}
	let (send, mut received) = mpsc::unbounded_channel();
	let mut streams = Vec::new();
	let ws = fixture.workspaces[0];
	for (n, process) in processes.iter().enumerate() {
		streams.push(observe(process, &fixture, n, Some(ws), -1, send.clone()).await);
	}
	let warmup = fixture.emit(ws, 1).await;
	for frame in receive(&mut received, 3, 5).await {
		assert_eq!(frame.id, warmup.sequence);
	}
	tokio::time::sleep(Duration::from_millis(500)).await;
	let before = futures_util::future::join_all(processes.iter().map(Process::metrics)).await;
	let response = fixture
		.f
		.client
		.clone()
		.patch(format!("{}/api/workspaces/{ws}", processes[0].endpoint))
		.bearer_auth(&fixture.token)
		.json(&json!({"revision":0,"state":{"boundary":"A-to-B-to-C"}}))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 200);
	let frames = receive(&mut received, 3, 3).await;
	for frame in &frames {
		assert_eq!(frame.data["data"]["state"]["boundary"], "A-to-B-to-C");
	}
	let after = futures_util::future::join_all(processes.iter().map(Process::metrics)).await;
	// Act: launch each replica after applying its broker availability or role configuration.
	for n in 0..3 {
		for reason in ["fallback", "initial", "reconnect"] {
			assert_eq!(cause(&before[n], reason), cause(&after[n], reason));
		}
		assert!(cause(&after[n], "notification") > cause(&before[n], "notification"));
	}
	// This inspection connection performs no pull request; B is the only
	// authorized process that can acknowledge this exact business event.
	let bus = aidash_server::bus::EventBus::connect(
		&format!("nats://executor:fixture@{host}:{port}"),
		&fixture.f.config.node_id,
	)
	.await
	.unwrap();
	let stream = bus.context.get_stream(&bus.stream_name).await.unwrap();
	let event = stream
		.get_last_raw_message_by_subject(&bus.subject)
		.await
		.unwrap();
	let envelope: Value = serde_json::from_slice(&event.payload).unwrap();
	assert_eq!(envelope["id"], frames[0].data["id"]);
	let mut execution = stream
		.get_consumer::<async_nats::jetstream::consumer::pull::Config>("execution")
		.await
		.unwrap();
	tokio::time::timeout(Duration::from_secs(3), async {
		while execution.info().await.unwrap().ack_floor.stream_sequence < event.sequence {
			tokio::time::sleep(Duration::from_millis(20)).await;
		}
	})
	.await
	.unwrap();
	std::fs::write(dir.join("assertions.json"), serde_json::to_vec_pretty(&json!({"writer":"A","exclusive_execution_consumer":"B","sse_observers":["A","B","C"],"notification_only":true,"exact_event_acknowledged":true})).unwrap()).unwrap();
	drop(streams);
	for process in &mut processes {
		process.stop().await;
	}
	drop(processes);
	fixture.finish().await;
}

#[fixture]
async fn outage_broker() -> reinhardt::test::testcontainers::ContainerAsync<GenericImage> {
	// reinhardt-web#6656: these scenarios need JetStream and isolated broker lifecycle or permissions.
	GenericImage::new("nats", "2.12-alpine")
		.with_exposed_port(4222.tcp())
		.with_cmd(["-js"])
		.start()
		.await
		.unwrap()
}

#[fixture]
async fn restricted_broker() -> reinhardt::test::testcontainers::ContainerAsync<GenericImage> {
	// reinhardt-web#6656: these scenarios need JetStream and isolated broker lifecycle or permissions.
	let config = br#"port: 4222
jetstream: {}
authorization {
  users: [
    {user: observer, password: fixture, permissions: {publish: {allow: [">"], deny: ["$JS.API.CONSUMER.MSG.NEXT.*.execution"]}, subscribe: [">"]}},
    {user: executor, password: fixture}
  ]
}
"#;
	GenericImage::new("nats", "2.12-alpine")
		.with_exposed_port(4222.tcp())
		.with_copy_to("/etc/nats/sse-test.conf", config.to_vec())
		.with_cmd(["--config", "/etc/nats/sse-test.conf"])
		.start()
		.await
		.unwrap()
}

struct OutageTransport {
	broker: reinhardt::test::testcontainers::ContainerAsync<GenericImage>,
	proxy: Proxy,
	broker_url: String,
}
#[fixture]
async fn outage_transport(
	#[future(awt)] outage_broker: reinhardt::test::testcontainers::ContainerAsync<GenericImage>,
) -> OutageTransport {
	let url = format!(
		"nats://{}:{}",
		outage_broker.get_host().await.unwrap(),
		outage_broker.get_host_port_ipv4(4222.tcp()).await.unwrap()
	);
	let proxy = Proxy::new(&url, false).await;
	OutageTransport {
		broker: outage_broker,
		proxy,
		broker_url: url,
	}
}

type DirectoryFuture =
	futures_util::future::Shared<futures_util::future::BoxFuture<'static, PathBuf>>;
#[fixture]
fn evidence_directory(
	#[default("sse".into())] label: String,
	#[from(sse_fixture)] root: SseFuture,
) -> DirectoryFuture {
	async move {
		let fixture = root.await;
		{
			// Preserve evidence beyond fixture teardown for the process acceptance scripts.
			let base = std::env::var_os("AIDASH_SSE_EVIDENCE_DIR")
				.map(PathBuf::from)
				.unwrap_or_else(|| std::env::temp_dir().join("aidash-sse-evidence"));
			let dir = base.join(format!("{label}-{}", fixture.schema));
			std::fs::create_dir_all(&dir).unwrap();
			dir
		}
	}
	.boxed()
	.shared()
}
#[fixture]
fn process_binary() -> PathBuf {
	candidate_binary()
}
#[fixture]
async fn initial_process(
	#[from(sse_fixture)] root: SseFuture,
	evidence_directory: DirectoryFuture,
	process_binary: PathBuf,
) -> Process {
	let fixture = root.await;
	let directory = evidence_directory.await;
	let mut process = Process::start(&fixture, &directory, 0, 60000, &process_binary);
	process.ready(&fixture, true).await;
	process
}
