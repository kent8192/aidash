//! Runs the actual server executable with deterministic before/after SQL cuts.
use super::*;
use reinhardt::test::fixtures::temp_dir;
#[cfg(unix)]
use std::os::{fd::AsRawFd, unix::process::CommandExt};
use std::{
	fs::File,
	path::Path,
	process::{Child, Command, Stdio},
	time::{Duration as WallDuration, Instant},
};
use tempfile::TempDir;

struct Server {
	process: Child,
	directory: Arc<TempDir>,
}
impl Server {
	fn start(node: &Node, cut: Option<(Uuid, &str, &Path)>, directory: Arc<TempDir>) -> Self {
		let listener = node
			.reservation
			.as_deref()
			.expect("real-process fixtures must retain their listener reservation");
		Self::start_with_listener(node, cut, Some(listener), directory)
	}
	fn start_with_listener(
		node: &Node,
		cut: Option<(Uuid, &str, &Path)>,
		listener: Option<&std::net::TcpListener>,
		directory: Arc<TempDir>,
	) -> Self {
		assert!(
			node.server.is_none(),
			"stop the in-process server before handover"
		);
		let log = File::create(directory.path().join("server.log")).unwrap();
		let binary = std::env::var_os("AIDASH_TEST_BINARY")
			.unwrap_or_else(|| env!("CARGO_BIN_EXE_aidash").into());
		// On macOS, dyld can stall before main when loading a large executable
		// from an external target volume. The process directory owns this local
		// snapshot and retains it until the child is killed and reaped.
		#[cfg(target_os = "macos")]
		let binary = {
			let snapshot = directory.path().join("aidash");
			std::fs::copy(&binary, &snapshot).expect("snapshot the exact process test executable");
			snapshot
		};
		let mut command = Command::new(binary);
		command
			.args(common::native_process_args(&node.f, "server"))
			.env_clear()
			.env("PATH", std::env::var_os("PATH").unwrap_or_default())
			.envs(common::native_process_environment(
				&node.f,
				&node.f.config.database_url,
				directory.path(),
				0,
			))
			.env(
				"AIDASH_SECRET_TEST_PEER",
				"local-peer-regression-test-token-0123456789",
			)
			.env("RUST_LOG", "off")
			.env("AIDASH_API_RATE_BURST", "100000")
			.env("AIDASH_AUTH_RATE_BURST", "100000")
			.stdin(Stdio::null())
			.stdout(log.try_clone().unwrap())
			.stderr(log);
		if let Some((id, point, directory)) = cut {
			command
				.env("AIDASH_TRANSACTION_FAULT", format!("{id}:{point}"))
				.env("AIDASH_TRANSACTION_FAULT_DIR", directory);
		}
		#[cfg(unix)]
		if let Some(listener) = listener {
			let fd = listener.as_raw_fd();
			command.env("AIDASH_LISTEN_FD", fd.to_string());
			// SAFETY: only async-signal-safe fcntl calls run after fork. Clearing
			// CLOEXEC only in this child leaves concurrent parent spawns isolated.
			// The node retains the descriptor until spawn completes and thereafter.
			unsafe {
				command.pre_exec(move || {
					let flags = libc::fcntl(fd, libc::F_GETFD);
					if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
						return Err(std::io::Error::last_os_error());
					}
					Ok(())
				});
			}
		}
		#[cfg(not(unix))]
		assert!(listener.is_none(), "listener handover requires Unix");
		Self {
			process: command.spawn().unwrap_or_else(|error| {
				panic!(
					"spawn {} on {}: {error}",
					node.f.config.node_id, node.listen
				)
			}),
			directory,
		}
	}
	fn log(&self) -> String {
		std::fs::read_to_string(self.directory.path().join("server.log")).unwrap()
	}
}
impl Drop for Server {
	fn drop(&mut self) {
		let _ = self.process.kill();
		let _ = self.process.wait();
	}
}
async fn healthy(node: &Node, server: &mut Server, stage: &str) {
	// Two independent debug executables cold-start together. Their startup can
	// exceed 20 seconds on macOS while dyld loads their local snapshots; this
	// allowance does not change the durable-cut or convergence deadlines.
	let ready = tokio::time::timeout(WallDuration::from_secs(60), async {
		loop {
			let exited = server.process.try_wait().unwrap();
			assert!(
				exited.is_none(),
				"server {} pid {} on {} exited {exited:?} during {stage}: {}",
				node.f.config.node_id,
				server.process.id(),
				node.listen,
				server.log()
			);
			if node
				.f
				.client
				.get(format!("{}/health", node.f.config.endpoint))
				.timeout(WallDuration::from_millis(300))
				.send()
				.await
				.is_ok_and(|r| r.status().is_success())
			{
				break;
			}
			tokio::time::sleep(WallDuration::from_millis(25)).await;
		}
	})
	.await;
	assert!(
		ready.is_ok(),
		"server startup timed out at {stage} for {} on {}: {}",
		node.f.config.node_id,
		node.listen,
		server.log()
	);
}

#[cfg(unix)]
struct HandoverNodes {
	nodes: [Node; 2],
	_environment: Arc<TestEnvironment>,
}

#[cfg(unix)]
#[rstest::fixture]
async fn handover_nodes(
	#[from(common::isolated_test_environment)] _environment: EnvironmentFuture,
	#[from(node)]
	#[with("handover-a",_environment.clone(),true)]
	first: BoxFuture<'static, Node>,
	#[from(node)]
	#[with("handover-b",_environment.clone(),true)]
	second: BoxFuture<'static, Node>,
) -> HandoverNodes {
	let (a, b) = tokio::join!(first, second);
	HandoverNodes {
		nodes: [a, b],
		_environment: _environment.await,
	}
}

#[cfg(unix)]
struct PortContender {
	task: Option<std::thread::JoinHandle<usize>>,
	stopping: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(unix)]
impl PortContender {
	fn start(addresses: [std::net::SocketAddr; 2]) -> Self {
		let stopping = Arc::new(std::sync::atomic::AtomicBool::new(false));
		let stop = stopping.clone();
		// This OS thread also competes while the test's Tokio thread is spawning
		// or reaping children, including the entire in-process handover interval.
		let task = std::thread::spawn(move || {
			let mut attempts = 0;
			while !stop.load(std::sync::atomic::Ordering::Relaxed) {
				for address in addresses {
					let error = std::net::TcpListener::bind(address)
						.expect_err("the fixture must retain its endpoint during handover");
					assert_eq!(
						error.kind(),
						std::io::ErrorKind::AddrInUse,
						"{address}: {error}"
					);
					attempts += 1;
				}
				std::thread::sleep(WallDuration::from_millis(1));
			}
			attempts
		});
		Self {
			task: Some(task),
			stopping,
		}
	}
	fn finish(mut self) {
		self.stopping
			.store(true, std::sync::atomic::Ordering::Relaxed);
		let attempts = self
			.task
			.take()
			.unwrap()
			.join()
			.expect("concurrent port contender");
		assert!(
			attempts >= 2,
			"the competing fixture must attempt both ports"
		);
		eprintln!("PORT-HANDOVER competing_bind_attempts={attempts}");
	}
}

#[cfg(unix)]
impl Drop for PortContender {
	fn drop(&mut self) {
		self.stopping
			.store(true, std::sync::atomic::Ordering::Relaxed);
		if let Some(task) = self.task.take() {
			let _ = task.join();
		}
	}
}

#[cfg(unix)]
#[rstest::rstest]
#[tokio::test]
async fn port_reservation_survives_concurrent_handover_and_process_restarts(
	#[future(awt)] handover_nodes: HandoverNodes,
) {
	let HandoverNodes {
		nodes: [mut a, mut b],
		_environment,
	} = handover_nodes;
	let endpoints = [a.f.config.endpoint.clone(), b.f.config.endpoint.clone()];
	assert_ne!(
		a.listen, b.listen,
		"concurrent fixtures need isolated endpoints"
	);
	// Act: compete for both endpoints throughout every process transition.
	let contender = PortContender::start([a.listen, b.listen]);
	for generation in 0..3 {
		assert_eq!(a.get("/health").await.0, 200);
		assert_eq!(b.get("/health").await.0, 200);
		a.stop().await;
		b.stop().await;
		// Act: both real executables inherit their independently owned sockets.
		// Each per-generation launch owns a fresh log and executable snapshot directory.
		let (directory_a, directory_b) = (Arc::new(temp_dir()), Arc::new(temp_dir()));
		let (mut process_a, mut process_b) = std::thread::scope(|scope| {
			let a = scope.spawn(|| Server::start(&a, None, directory_a));
			let b = scope.spawn(|| Server::start(&b, None, directory_b));
			(a.join().unwrap(), b.join().unwrap())
		});
		healthy(&a, &mut process_a, "concurrent handover").await;
		healthy(&b, &mut process_b, "concurrent handover").await;
		assert_eq!(a.f.config.endpoint, endpoints[0]);
		assert_eq!(b.f.config.endpoint, endpoints[1]);
		// Act: reap both SIGKILLed children before rebuilding the native nodes.
		drop(process_a);
		drop(process_b);
		if generation < 2 {
			a.restart().await;
			b.restart().await;
		}
	}
	contender.finish();
	a.cleanup().await;
	b.cleanup().await;
}

#[cfg(unix)]
#[rstest::rstest]
#[case::occupied_port(false, "HTTP listener bind")]
#[case::wrong_listener(true, "inherited HTTP listener")]
#[tokio::test]
async fn child_listener_conflicts_fail_with_endpoint_diagnostics(
	#[future(awt)] handover_nodes: HandoverNodes,
	#[case] wrong_listener: bool,
	#[case] diagnostic: &str,
	#[from(process_directory)] directory: Arc<TempDir>,
) {
	let HandoverNodes {
		nodes: [mut a, mut b],
		_environment,
	} = handover_nodes;
	a.stop().await;
	b.stop().await;
	let mut server = Server::start_with_listener(
		&a,
		None,
		wrong_listener.then(|| b.reservation.as_deref().unwrap()),
		directory,
	);
	let status = tokio::time::timeout(WallDuration::from_secs(60), async {
		loop {
			if let Some(status) = server.process.try_wait().unwrap() {
				break status;
			}
			tokio::time::sleep(WallDuration::from_millis(25)).await;
		}
	})
	.await
	.unwrap_or_else(|_| panic!("conflicting child did not exit: {}", server.log()));
	assert!(!status.success(), "a listener conflict must fail startup");
	let log = server.log();
	assert!(log.contains(diagnostic), "{log}");
	assert!(log.contains(&a.listen.to_string()), "{log}");
	if wrong_listener {
		assert!(log.contains(&b.listen.to_string()), "{log}");
		assert!(log.contains("expected"), "{log}");
	} else {
		assert!(log.contains("Address already in use"), "{log}");
	}
	drop(server);
	a.cleanup().await;
	b.cleanup().await;
}

#[rstest::rstest]
#[case::submit("coordinator.submit", false)]
#[case::vote("coordinator.vote", false)]
#[case::commit("coordinator.commit", false)]
#[case::abort_decision("coordinator.abort", true)]
#[case::visible("coordinator.visible", false)]
#[case::complete_commit("coordinator.complete", false)]
#[case::complete_abort("coordinator.complete", true)]
#[case::reserve("participant.reserve", false)]
#[case::prepare("participant.prepare", false)]
#[case::apply("participant.apply", false)]
#[case::release("participant.release", false)]
#[case::abort_participant("participant.abort", true)]
#[tokio::test]
async fn real_server_sigkill_at_durable_cut(
	#[case] _phase: &str,
	#[case] abort: bool,
	#[values("before", "after")] _edge: &str,
	#[values(1, 2, 3)] repetition: usize,
	#[future(awt)]
	#[from(cut_fixture)]
	#[with(_phase, abort, _edge)]
	fixture: CutFixture,
) {
	// Every cut is repeated three times as independent cases; any failed repetition fails CI.
	let CutFixture {
		pair: (a, b, manifest, wa, wb),
		directory,
		mut process_a,
		mut process_b,
		replacements,
		point,
		participant,
	} = fixture;
	let client = a.f.client.clone();
	let endpoint = a.f.config.endpoint.clone();
	let token = a.f.config.api_token.clone();
	let body = json!(manifest);
	let submission = tokio::spawn(async move {
		client
			.post(format!("{endpoint}/api/transactions"))
			.bearer_auth(token)
			.json(&body)
			.send()
			.await
	});
	let marker = directory
		.path()
		.join(format!("{}.{point}.reached", manifest.id));
	let mut observations = 0;
	tokio::time::timeout(WallDuration::from_secs(40), async {
		loop {
			for (node, workspace) in [(&a, wa), (&b, wb)] {
				let response = node
					.f
					.client
					.get(format!(
						"{}/api/workspaces/{workspace}",
						node.f.config.endpoint
					))
					.bearer_auth(&node.f.config.api_token)
					.timeout(WallDuration::from_secs(5))
					.send()
					.await
					.unwrap();
				match response.status().as_u16() {
					200 => {
						let value: Value = response.json().await.unwrap();
						let revision = value["workspace"]["revision"]
							.as_i64()
							.expect("workspace snapshot revision");
						assert!(matches!(revision, 0 | 1));
						if revision == 1 {
							assert_eq!(a.f.store.workspace(wa).await.unwrap().revision, 1);
							assert_eq!(b.f.store.workspace(wb).await.unwrap().revision, 1);
						}
					}
					503 => {}
					status => panic!("unexpected visibility status {status}"),
				}
				observations += 1;
			}
			if marker.exists() {
				break;
			}
			tokio::time::sleep(WallDuration::from_millis(15)).await;
		}
	})
	.await
	.unwrap_or_else(|_| {
		panic!(
			"unreached cut {point}, repetition {repetition}; server A: {}; server B: {}",
			process_a.as_ref().unwrap().log(),
			process_b.as_ref().unwrap().log()
		)
	});
	// Observe at the held cut, then kill only the owning OS process. The
	// independent peer and both databases retain their current state.
	assert!(observations >= 2);
	if participant {
		drop(process_b.take());
	} else {
		drop(process_a.take());
	}
	submission.abort();
	let _ = submission.await;
	if participant {
		process_b = Some(Server::start(&b, None, replacements.1.clone()));
		healthy(&b, process_b.as_mut().unwrap(), "restored").await;
	} else {
		process_a = Some(Server::start(&a, None, replacements.0.clone()));
		healthy(&a, process_a.as_mut().unwrap(), "restored").await;
	}
	let restored = Instant::now();
	// Submission-before-commit legitimately left no coordinator record.
	let (status, body) = a
		.request(
			reqwest::Method::POST,
			"/api/transactions",
			Some(json!(manifest)),
		)
		.await;
	assert_eq!(status, 202, "{body}");
	tokio::time::timeout(WallDuration::from_secs(180), async {
		loop {
			if coordinator::status(&a.f, manifest.id)
				.await
				.unwrap()
				.complete
			{
				break;
			}
			tokio::time::sleep(WallDuration::from_millis(25)).await;
		}
	})
	.await
	.expect("convergence after service restoration");
	let result = coordinator::status(&a.f, manifest.id).await.unwrap();
	assert_eq!(
		result.decision.as_deref(),
		Some(if abort { "ABORT" } else { "COMMIT" })
	);
	for (node, workspace) in [(&a, wa), (&b, wb)] {
		let expected = i64::from(!abort);
		assert_eq!(
			node.f.store.workspace(workspace).await.unwrap().revision,
			expected
		);
		assert_eq!(
			node.get(&format!("/api/workspaces/{workspace}")).await.0,
			200
		);
		let count: i64 = {
			let query_bind_1 = workspace;
			sqlx::query_scalar(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::cust("COUNT(*)"))
					.from(reinhardt::query::Alias::new("events"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(workspace_id=? AND kind='workspace.updated')".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(node.f.store.pool.driver())
			.await
		}
		.unwrap();
		assert_eq!(count, expected, "logical event applied exactly once");
	}
	eprintln!(
		"TX-PROCESS point={point} abort={abort} repetition={repetition} converged_ms={} observations={observations}",
		restored.elapsed().as_millis()
	);
	drop(process_a);
	drop(process_b);
	a.cleanup().await;
	b.cleanup().await;
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::SimpleExpr;

use reinhardt::query::Expr;

#[fixture]
fn process_directory(temp_dir: TempDir) -> Arc<TempDir> {
	Arc::new(temp_dir)
}
struct ProcessDirectories {
	fault: Arc<TempDir>,
	first: Arc<TempDir>,
	second: Arc<TempDir>,
	replacements: (Arc<TempDir>, Arc<TempDir>),
}
#[fixture]
fn process_directories(
	#[from(process_directory)] fault: Arc<TempDir>,
	#[from(process_directory)] first: Arc<TempDir>,
	#[from(process_directory)] second: Arc<TempDir>,
	#[from(process_directory)] replacement_a: Arc<TempDir>,
	#[from(process_directory)] replacement_b: Arc<TempDir>,
) -> ProcessDirectories {
	ProcessDirectories {
		fault,
		first,
		second,
		replacements: (replacement_a, replacement_b),
	}
}
/// Real-process pairs retain both listener reservations across handover and SIGKILL.
#[fixture]
fn isolated_pair(
	#[from(common::isolated_test_environment)] _environment: EnvironmentFuture,
	#[from(pair)]
	#[with(_environment.clone(),true)]
	pair: PairFuture,
) -> PairFuture {
	pair
}
struct CutFixture {
	pair: Pair,
	directory: Arc<TempDir>,
	process_a: Option<Server>,
	process_b: Option<Server>,
	replacements: (Arc<TempDir>, Arc<TempDir>),
	point: String,
	participant: bool,
}
#[fixture]
async fn cut_fixture(
	#[default("coordinator.submit")] phase: &str,
	#[default(false)] abort: bool,
	#[default("before")] edge: &str,
	#[future(awt)] isolated_pair: Pair,
	process_directories: ProcessDirectories,
) -> CutFixture {
	let (mut a, mut b, mut manifest, wa, wb) = isolated_pair;
	if abort {
		let aidash_server::transactions::Mutation::WorkspaceState {
			expected_revision, ..
		} = &mut manifest.participants[1].mutations[0]
		else {
			unreachable!()
		};
		*expected_revision = 99;
	}
	a.stop().await;
	b.stop().await;
	let directories = process_directories;
	let directory = directories.fault;
	let point = format!("{phase}.{edge}");
	let participant = phase.starts_with("participant.");
	let mut process_a = Server::start(
		&a,
		(!participant).then_some((manifest.id, point.as_str(), directory.path())),
		directories.first,
	);
	let mut process_b = Server::start(
		&b,
		participant.then_some((manifest.id, point.as_str(), directory.path())),
		directories.second,
	);
	healthy(&a, &mut process_a, "initial").await;
	healthy(&b, &mut process_b, "initial").await;
	CutFixture {
		pair: (a, b, manifest, wa, wb),
		directory,
		process_a: Some(process_a),
		process_b: Some(process_b),
		replacements: directories.replacements,
		point,
		participant,
	}
}
