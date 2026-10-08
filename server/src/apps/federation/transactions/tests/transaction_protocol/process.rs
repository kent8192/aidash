//! Runs the actual server executable with deterministic before/after SQL cuts.
use super::*;
use reinhardt::test::fixtures::temp_dir;
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
		Self {
			process: command.spawn().unwrap(),
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
async fn healthy(node: &Node, server: &mut Server) {
	let ready = tokio::time::timeout(WallDuration::from_secs(20), async {
		loop {
			assert!(
				server.process.try_wait().unwrap().is_none(),
				"server exited during startup: {}",
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
	assert!(ready.is_ok(), "server startup timed out: {}", server.log());
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
	.unwrap_or_else(|_| panic!("unreached cut {point}, repetition {repetition}"));
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
		healthy(&b, process_b.as_mut().unwrap()).await;
	} else {
		process_a = Some(Server::start(&a, None, replacements.0.clone()));
		healthy(&a, process_a.as_mut().unwrap()).await;
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
#[fixture]
fn isolated_pair(
	#[from(common::isolated_test_environment)] _environment: EnvironmentFuture,
	#[from(pair)]
	#[with(_environment.clone())]
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
	healthy(&a, &mut process_a).await;
	healthy(&b, &mut process_b).await;
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
