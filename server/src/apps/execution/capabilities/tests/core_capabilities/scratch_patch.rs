use super::*;
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Clone, Default)]
struct PatchDispatch {
	entered: Arc<Notify>,
	release: Arc<Notify>,
}

#[rstest::fixture]
fn patch_dispatch() -> PatchDispatch {
	PatchDispatch::default()
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
		.env("AIDASH_SCRATCH_PATCH_TEST_CHILD", "1")
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

#[rstest::fixture]
fn patch_runner(patch_dispatch: PatchDispatch) -> Arc<reinhardt::ServerRouter> {
	let PatchDispatch { entered, release } = patch_dispatch;
	let receipt = std::sync::Mutex::new(None);
	Arc::new(
		reinhardt::test::stub::StubRouter::new()
			.post("/v2/files/patch", move |request: reinhardt::Request| {
				let input = request.json::<Value>().unwrap();
				let apply = input["apply"].as_bool().expect("Runner patch mode");
				let claim: Uuid = serde_json::from_value(input["claim_id"].clone()).unwrap();
				let mut saved = receipt.lock().unwrap();
				if apply {
					assert!(saved.replace(claim).is_none(), "patch was dispatched twice");
				} else {
					assert_eq!(*saved, Some(claim));
				}
				let (entered, release) = (entered.clone(), release.clone());
				async move {
					if apply {
						entered.notify_one();
						release.notified().await;
						// The helper applied the patch and retained its receipt, but
						// the original exec lost its response.
						Ok(reinhardt::Response::new(reinhardt::StatusCode::CONFLICT)
							.with_json(&json!({"code":"file_helper_failed","message":"lost reply"}))
							.unwrap())
					} else {
						Ok(reinhardt::Response::ok()
							.with_json(&json!({"changed":["notes.txt"]}))
							.unwrap())
					}
				}
			})
			.into_server_router(),
	)
}

#[rstest::rstest]
#[tokio::test]
async fn scratch_patch_claim_survives_runner_reply_loss(
	patch_dispatch: PatchDispatch,
	#[from(patch_runner)]
	#[with(patch_dispatch.clone())]
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
	if std::env::var_os("AIDASH_SCRATCH_PATCH_TEST_CHILD").is_none() {
		with_runner_profile().await;
		return;
	}
	let c = core.await;
	let run = admit(&c).await;
	let path = format!("/api/executions/{}/patch", run.id);
	let input = json!({
		"idempotency_key":Uuid::new_v4(), "expected_revision":1, "preconditions":{},
		"patch":"*** Begin Patch\n*** Add File: notes.txt\n+once\n*** End Patch",
	});
	{
		let first = request(&c.app, &c.token, "POST", &path, input.clone());
		tokio::pin!(first);
		tokio::select! {
			result = &mut first => panic!("patch ended before dispatch: {result:?}"),
			result = tokio::time::timeout(Duration::from_secs(15), patch_dispatch.entered.notified()) => {
				result.expect("patch did not reach the Runner");
			}
		}
		let mut different = input.clone();
		different["patch"] =
			json!("*** Begin Patch\n*** Add File: notes.txt\n+different\n*** End Patch");
		let (status, conflict) = tokio::time::timeout(
			Duration::from_secs(10),
			request(&c.app, &c.token, "POST", &path, different),
		)
		.await
		.unwrap();
		assert_eq!(status, 409, "{conflict}");
		assert_eq!(conflict["error"]["code"], "IDEMPOTENCY_CONFLICT");

		patch_dispatch.release.notify_one();
		let (status, lost) = first.as_mut().await;
		assert_eq!(status, 409, "{lost}");
		assert_eq!(lost["error"]["code"], "FILE_HELPER_FAILED");
	}
	let (status, recovered) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{recovered}");
	assert_eq!(recovered["changed"], json!(["notes.txt"]));
	let (status, replayed) = request(&c.app, &c.token, "POST", &path, input).await;
	assert_eq!(status, 200, "{replayed}");
	assert_eq!(replayed["changed"], recovered["changed"]);
	c.close().await;
}
