use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Clone, Default)]
struct StreamingSave {
	entered: Arc<Notify>,
	release: Arc<Notify>,
}

#[rstest::fixture]
fn streaming_save() -> StreamingSave {
	StreamingSave::default()
}

#[rstest::fixture]
fn scratch_runner(streaming_save: StreamingSave) -> Arc<reinhardt::ServerRouter> {
	let StreamingSave { entered, release } = streaming_save;
	let requests = AtomicUsize::new(0);
	let key = Uuid::new_v4();
	let router = reinhardt::test::stub::StubRouter::new()
		.post(
			"/v1/chat/completions",
			move |request: reinhardt::Request| {
				let input = request.json::<Value>().unwrap();
				let first = requests.fetch_add(1, Ordering::SeqCst) == 0;
				if first {
					assert!(
						input["tools"]
							.as_array()
							.unwrap()
							.iter()
							.any(|tool| tool["function"]["name"] == "file_save")
					);
				}
				async move {
					let (reason, message) = if first {
						let arguments = json!({
							"paths":["hello.txt"],"expected_revision":1,"idempotency_key":key,
						});
						(
							"tool_calls",
							json!({"role":"assistant","content":null,"tool_calls":[{
								"id":"save","type":"function","function":{
									"name":"file_save","arguments":arguments.to_string(),
								},
							}]}),
						)
					} else {
						("stop", json!({"role":"assistant","content":"Done."}))
					};
					Ok(reinhardt::Response::ok()
						.with_json(
							&json!({"choices":[{"index":0,"finish_reason":reason,"message":message}],
						"usage":{"prompt_tokens":1,"completion_tokens":1}}),
						)
						.unwrap())
				}
			},
		)
		.post("/v2/files/manifest", |_| async {
			let digest = aidash_server::capabilities::objects::digest(b"hello");
			Ok(reinhardt::Response::ok()
				.with_json(
					&json!({"files":[{"path":"hello.txt","size":5,"sha256":digest}],
					"skipped_symlinks":[],"truncated":false}),
				)
				.unwrap())
		})
		.post("/v2/files/read", move |_| {
			let (entered, release) = (entered.clone(), release.clone());
			async move {
				entered.notify_one();
				release.notified().await;
				Ok(reinhardt::Response::ok()
					.with_json(&json!({"data":"aGVsbG8=","size":5,"next_offset":5,"eof":true}))
					.unwrap())
			}
		})
		.into_server_router();
	Arc::new(router)
}

// Registry provider availability reads the Deployment profile from the process
// environment. Give this test its own profile so it can seed the Scratch tools.
async fn with_runner_profile() {
	let root = tempfile::tempdir().unwrap();
	let profile = root.path().join("profile.json");
	std::fs::write(
		&profile,
		json!({
			"admission":true, "storage":root.path().join("storage"),
			"runner":{"endpoint":"http://127.0.0.1:19999","token_env":"AIDASH_COMMAND_CLEANUP_TEST_TOKEN",
				"image":"stub","runtime_class":"stub","namespace":"stub"},
		})
		.to_string(),
	)
	.unwrap();
	let thread = std::thread::current();
	let test = thread.name().expect("libtest names its test thread");
	let output = tokio::process::Command::new(std::env::current_exe().unwrap())
		.args(["--exact", test])
		.env("AIDASH_CAPABILITY_PROFILE", profile)
		.env("AIDASH_SCRATCH_SAVE_TEST_CHILD", "1")
		.env("AIDASH_POLICY_WAIT_SECONDS", "10")
		.env(
			"AIDASH_COMMAND_CLEANUP_TEST_TOKEN",
			"aidash-command-cleanup-fixture",
		)
		.output()
		.await
		.unwrap();
	let stdout = String::from_utf8_lossy(&output.stdout);
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(
		output.status.success() && stdout.contains("1 passed"),
		"{stdout}{stderr}"
	);
}

#[rstest::rstest]
#[case::revoked(true)]
#[case::unchanged(false)]
#[tokio::test]
async fn scratch_save_releases_authority_while_streaming(
	#[case] revoke: bool,
	streaming_save: StreamingSave,
	#[from(scratch_runner)]
	#[with(streaming_save.clone())]
	_runner: Arc<reinhardt::ServerRouter>,
	#[from(upstream_fixtures::provider_transport)]
	#[with(_runner.clone())]
	_transport: upstream_fixtures::UpstreamFuture,
	#[from(provider_endpoint)]
	#[with(_transport.clone())]
	_endpoint: EndpointFuture,
	#[from(capability_fixture)]
	#[with("aidash://execution-test",_endpoint.clone(),FixtureRunner::Stub(_endpoint.clone()))]
	core: CoreFuture,
) {
	if std::env::var_os("AIDASH_SCRATCH_SAVE_TEST_CHILD").is_none() {
		with_runner_profile().await;
		return;
	}
	let c = core.await;
	let run = admit(&c).await;
	let harness = aidash_server::harness::Harness {
		federation: c.f.clone(),
	};
	let run_id = run.id;
	let worker = async move {
		loop {
			let worked = harness.worker_once().await.unwrap();
			let current = harness
				.federation
				.store
				.run(&TEST_FIXTURE, run_id)
				.await
				.unwrap();
			if current.phase().is_terminal() || !worked {
				return current;
			}
		}
	};
	tokio::pin!(worker);
	tokio::select! {
		current = &mut worker => panic!("Run ended before streaming: {:?} {:?} {:?}", current.error, current.state, current.context.events().filter(|event| matches!(event, aidash_server::context::ContextEvent::Tool { .. })).collect::<Vec<_>>()),
		result = tokio::time::timeout(Duration::from_secs(45), streaming_save.entered.notified()) => {
			result.expect("file_save did not reach the Runner");
		}
	}

	if revoke {
		let mut policy = c.policy.clone();
		policy["policies"].as_array_mut().unwrap().push(json!({
			"id":"withdraw-file-write","effect":"deny","subjects":{"any":true},
			"actions":["file.write"],"resources":{"kinds":["working_area"]},
		}));
		let change = request(
			&c.app,
			&c.f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":2,"bundle":policy}),
		);
		// Keep polling the worker while the Runner withholds its response.
		let changed = tokio::select! {
			current = &mut worker => panic!("Run ended before Runner release: {:?} {:?}", current.error, current.state),
			result = tokio::time::timeout(Duration::from_secs(12), change) => result,
		};
		let (status, body) = changed.expect("Authority Change waited on the blocked Runner");
		assert_eq!(status, 200, "Authority Change while streaming: {body}");
	}
	streaming_save.release.notify_one();
	let current = tokio::time::timeout(Duration::from_secs(45), worker)
		.await
		.expect("worker did not settle the tool");
	assert!(
		current.context.events().any(|event| matches!(
			event, aidash_server::context::ContextEvent::Tool { call, result }
				if call.name == "file_save" && result["status"] == if revoke { "blocked" } else { "completed" }
		)),
		"{:?} {:?}",
		current.error,
		current.state
	);

	let (status, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/executions/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	if revoke {
		assert_eq!(area["revision"], 1);
		assert_eq!(area["manifest"], json!([]));
	} else {
		assert_eq!(area["revision"], 2);
		assert_eq!(area["manifest"][0]["path"], "hello.txt");
		assert_eq!(
			area["manifest"][0]["digest"],
			aidash_server::capabilities::objects::digest(b"hello")
		);
	}
	c.close().await;
}
