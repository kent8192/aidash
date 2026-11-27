use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Clone, Default)]
struct StreamingSave {
	verify_video: Arc<AtomicBool>,
	entered: Arc<Notify>,
	release: Arc<Notify>,
}

#[rstest::fixture]
fn streaming_save() -> StreamingSave {
	StreamingSave::default()
}

const VIDEO: &[u8] = include_bytes!(concat!(
	env!("CARGO_MANIFEST_DIR"),
	"/../web/tests/fixtures/preview.mp4"
));
const IMAGE: &[u8] = b"\x89PNG\r\n\x1a\npreview";

fn saved_fixture(verify_video: &AtomicBool) -> (&'static str, &'static [u8], &'static str) {
	if verify_video.load(Ordering::Relaxed) {
		("preview.mp4", VIDEO, "video/mp4")
	} else {
		("preview.png", IMAGE, "image/png")
	}
}

#[rstest::fixture]
fn scratch_runner(streaming_save: StreamingSave) -> Arc<reinhardt::ServerRouter> {
	let StreamingSave {
		entered, release, ..
	} = streaming_save;
	let behavior = streaming_save.verify_video.clone();
	let manifest_behavior = behavior.clone();
	let read_behavior = behavior.clone();
	let requests = AtomicUsize::new(0);
	let key = Uuid::new_v4();
	let router = reinhardt::test::stub::StubRouter::new()
		.post(
			"/v1/chat/completions",
			move |request: reinhardt::Request| {
				let input = request.json::<Value>().unwrap();
				let step = requests.fetch_add(1, Ordering::SeqCst);
				let (path, _, _) = saved_fixture(&behavior);
				if step == 0 {
					assert!(
						input["tools"]
							.as_array()
							.unwrap()
							.iter()
							.any(|tool| tool["function"]["name"] == "file_save")
					);
				}
				async move {
					let call = match (step, path) {
						(0, _) => Some((
							"file_save",
							json!({
								"paths":[path],"expected_revision":1,"idempotency_key":key,
							}),
						)),
						(1, "preview.mp4") => Some((
							"artifact_publish",
							json!({
								"kind":"file_reference","name":"Missing video","content":"missing.mp4",
							}),
						)),
						(2, "preview.mp4") => Some((
							"artifact_publish",
							json!({
								"kind":"file_reference","name":"Saved video","content":path,
							}),
						)),
						(3, "preview.mp4") => Some((
							"thread_message",
							json!({
								"content":"Here is the video.","files":[path],
							}),
						)),
						_ => None,
					};
					let (reason, message) = if let Some((name, arguments)) = call {
						(
							"tool_calls",
							json!({"role":"assistant","content":null,"tool_calls":[{
								"id":format!("call-{step}"),"type":"function","function":{
									"name":name,"arguments":arguments.to_string(),
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
		.post("/v2/files/manifest", move |_| {
			let (path, bytes, _) = saved_fixture(&manifest_behavior);
			async move {
				let digest = aidash_server::capabilities::objects::digest(bytes);
				Ok(reinhardt::Response::ok()
					.with_json(
						&json!({"files":[{"path":path,"size":bytes.len(),"sha256":digest}],
						"skipped_symlinks":[],"truncated":false}),
					)
					.unwrap())
			}
		})
		.post("/v2/files/read", move |_| {
			let (entered, release) = (entered.clone(), release.clone());
			let (_, bytes, _) = saved_fixture(&read_behavior);
			async move {
				entered.notify_one();
				release.notified().await;
				Ok(reinhardt::Response::ok()
					.with_json(&json!({"data":STANDARD.encode(bytes),"size":bytes.len(),
						"next_offset":bytes.len(),"eof":true}))
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

#[derive(Clone, Copy)]
enum Change {
	Policy,
	Membership,
	EntranceFlag,
	None,
}

#[rstest::rstest]
#[case::revoked_policy(Change::Policy)]
#[case::revoked_membership(Change::Membership)]
#[case::existing_work(Change::EntranceFlag)]
#[case::unchanged(Change::None)]
#[tokio::test]
async fn scratch_save_releases_authority_while_streaming(
	#[case] change: Change,
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
	streaming_save
		.verify_video
		.store(matches!(change, Change::None), Ordering::Relaxed);
	let c = core.await;
	let run = admit(&c).await;
	let membership = if matches!(change, Change::Membership) {
		let pool = c.f.store.pool.fixture_driver();
		let (space, starter): (Uuid, String) =
			sqlx::query_as("SELECT scratch_space_id, created_by FROM threads WHERE id = $1")
				.bind(run.thread_id)
				.fetch_one(pool)
				.await
				.unwrap();
		assert_ne!(starter, "bob");
		// Make the Thread starter a member of the existing dedicated Space.
		sqlx::query("UPDATE scratch_spaces SET creator = 'bob' WHERE id = $1")
			.bind(space)
			.execute(pool)
			.await
			.unwrap();
		sqlx::query(
			"INSERT INTO scratch_space_members (id, tenant, space_id, subject) VALUES ($1, 'acme', $2, $3)",
		)
		.bind(Uuid::new_v4())
		.bind(space)
		.bind(&starter)
		.execute(pool)
		.await
		.unwrap();
		let owner = common::issued_credential(&c.f.store, "acme", "bob").await;
		Some((space, starter, owner))
	} else {
		None
	};
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

	if matches!(change, Change::Policy) {
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
	if let Some((space, starter, owner)) = membership {
		let path = format!("/api/scratch-spaces/{space}/members/remove");
		let remove = request(
			&c.app,
			owner["token"].as_str().unwrap(),
			"POST",
			&path,
			json!({"subject":starter}),
		);
		let removed = tokio::select! {
			current = &mut worker => panic!("Run ended before Runner release: {:?} {:?}", current.error, current.state),
			result = tokio::time::timeout(Duration::from_secs(12), remove) => result,
		};
		let (status, body) = removed.expect("membership change waited on the blocked Runner");
		assert_eq!(status, 200, "{body}");
	}
	let flag = if matches!(change, Change::EntranceFlag) {
		let mut flag = c.f.store.pool.begin(&TEST_FIXTURE).await.unwrap();
		aidash_server::database::authority_change::raise(&mut *flag, "acme")
			.await
			.unwrap();
		Some(flag)
	} else {
		None
	};
	streaming_save.release.notify_one();
	let current = if let Some(flag) = flag {
		// Observe publication while the entrance is blocked. The next task
		// step may wait at that entrance, even after the save has settled.
		let published = async {
			loop {
				let revision: i64 =
					sqlx::query_scalar("SELECT revision FROM core_areas WHERE thread_id = $1")
						.bind(run.thread_id)
						.fetch_one(c.f.store.pool.fixture_driver())
						.await
						.unwrap();
				if revision == 2 {
					break;
				}
				tokio::time::sleep(Duration::from_millis(30)).await;
			}
		};
		let mut completed = None;
		tokio::time::timeout(Duration::from_secs(10), async {
			tokio::pin!(published);
			tokio::select! {
				() = &mut published => {},
				current = &mut worker => {
					completed = Some(current);
					published.await;
				}
			}
		})
		.await
		.expect("file settlement waited at the new-work entrance");
		flag.rollback().await.unwrap();
		completed
	} else {
		None
	};
	let current = match current {
		Some(current) => current,
		None => tokio::time::timeout(Duration::from_secs(45), worker)
			.await
			.expect("worker did not settle the tool"),
	};
	let revoke = matches!(change, Change::Policy | Change::Membership);
	if !matches!(change, Change::Membership) {
		assert!(
			current.context.events().any(|event| matches!(
				event, aidash_server::context::ContextEvent::Tool { call, result }
					if call.name == "file_save" && result["status"] == if revoke { "blocked" } else { "completed" }
			)),
			"{:?} {:?}",
			current.error,
			current.state
		);
	}

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
		let (path, bytes, media_type) = saved_fixture(&streaming_save.verify_video);
		let file = &area["manifest"][0];
		assert_eq!(file["path"], path);
		assert_eq!(file["media_type"], media_type);
		assert_eq!(
			file["digest"],
			aidash_server::capabilities::objects::digest(bytes)
		);
		let receipt = current
			.context
			.events()
			.find_map(|event| match event {
				aidash_server::context::ContextEvent::Tool { call, result }
					if call.name == "file_save" =>
				{
					Some(result)
				}
				_ => None,
			})
			.expect("file_save receipt");
		assert_eq!(receipt["saved"], json!([path]));
		assert_eq!(
			receipt["files"],
			json!([{
				"path": path, "file_id": file["file_id"], "digest": file["digest"],
			}])
		);
		let (status, result) = request(
			&c.app,
			&c.token,
			"POST",
			&format!("/api/executions/{}/files/search", run.id),
			json!({"query":path,"mode":"path","scope":"working"}),
		)
		.await;
		assert_eq!(status, 200, "{result}");
		assert_eq!(result["matches"][0]["file_id"], file["file_id"]);
		assert_eq!(result["matches"][0]["digest"], file["digest"]);
		if matches!(change, Change::None) {
			assert!(
				current.context.events().any(|event| matches!(
					event, aidash_server::context::ContextEvent::Tool { call, result }
						if call.name == "artifact_publish" && call.arguments["content"] == "missing.mp4"
							&& result.to_string().contains("file_save")
				)),
				"missing Scratch Space file should get actionable save instructions"
			);
			let (status, thread) = request(
				&c.app,
				&c.token,
				"GET",
				&format!("/api/threads/{}", run.thread_id),
				Value::Null,
			)
			.await;
			assert_eq!(status, 200, "{thread}");
			let artifacts = thread["artifacts"].as_array().unwrap();
			assert_eq!(artifacts.len(), 1, "{artifacts:?}");
			let artifact = &artifacts[0];
			assert_eq!(
				artifact["content"],
				json!({
					"area_id":area["id"],"file_id":file["file_id"],"path":path,
					"digest":file["digest"],"size":bytes.len(),"media_type":media_type,
				})
			);
			let hold: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM core_retention_holds h JOIN core_retention_files f ON f.hold_id = h.id WHERE h.holder_kind = 'artifact' AND h.holder_id = $1 AND f.file_id = $2)")
				.bind(artifact["id"].as_str().unwrap())
				.bind(serde_json::from_value::<Uuid>(file["file_id"].clone()).unwrap())
				.fetch_one(c.f.store.pool.fixture_driver())
				.await
				.unwrap();
			assert!(
				hold,
				"the published artifact must retain its exact File version"
			);
			let (status, messages) = request(
				&c.app,
				&c.token,
				"GET",
				&format!("/api/threads/{}/message-history", run.thread_id),
				Value::Null,
			)
			.await;
			assert_eq!(status, 200, "{messages}");
			assert!(
				messages["messages"]
					.as_array()
					.unwrap()
					.iter()
					.any(|message| message["attachments"]
						.as_array()
						.unwrap()
						.iter()
						.any(|attachment| attachment["filename"] == path
							&& attachment["media_type"] == media_type)),
				"{messages}"
			);
		}
	}
	c.close().await;
}

#[rstest::fixture]
fn patch_runner(streaming_save: StreamingSave) -> Arc<reinhardt::ServerRouter> {
	let StreamingSave {
		entered, release, ..
	} = streaming_save;
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
	streaming_save: StreamingSave,
	#[from(patch_runner)]
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
			result = tokio::time::timeout(Duration::from_secs(15), streaming_save.entered.notified()) => {
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

		streaming_save.release.notify_one();
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
