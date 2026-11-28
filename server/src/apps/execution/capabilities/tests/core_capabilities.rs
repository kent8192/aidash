#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../tests/support/legacy.rs"]
mod common;
#[path = "../../tests/support/late_subnode.rs"]
mod late_subnode;
use aidash_application::tenancy::TEST_FIXTURE;
use common::upstream_fixtures;
use common::{bootstrap, cleanup, request};
use futures_util::{
	FutureExt,
	future::{BoxFuture, Shared},
};
use serde_json::json;
use std::sync::Arc;

#[rstest::rstest]
#[tokio::test]
async fn working_files_require_explicit_agent_settings(
	#[future(awt)]
	#[from(common::native_application)]
	application: common::ApplicationFixture,
) {
	let (f, url, schema) = application.runtime.parts();
	let app = application.application;
	let (mut policy, token, task) = bootstrap(&f, &app, "http://localhost:19999").await;
	let mut stripped = f
		.registry
		.get(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			"research",
			"1.0.0",
		)
		.await
		.unwrap();
	stripped.version = "1.0.1".into();
	stripped.binding_normalization = None;
	for name in ["file_read", "file_search"] {
		let removed = stripped.config["remove_default"].as_array_mut().unwrap();
		if !removed.contains(&json!(name)) {
			removed.push(json!(name));
		}
	}
	f.registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			stripped,
		)
		.await
		.unwrap();
	policy["subjects"][aidash_server::domain::qualified_agent(
		&aidash_domain::nodes::NodeAddress::derived(f.config.deployment.as_str(), "acme"),
		"research",
		"1.0.1",
	)] = json!({"kind":"agent", "node": aidash_domain::nodes::NodeName::derived("acme")});
	common::replace_policy(&f.store, "acme", policy.clone()).await;
	let (status, body) = request(
		&app,
		&token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"research","version":"1.0.1"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status, delegation) = request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{task}/delegate"),
		json!({"node_id":f.config.deployment,"agent":{"id":"research","version":"1.0.1"}}),
	)
	.await;
	assert_eq!(status, 200, "{delegation}");
	let (status, result) = request(&app, &token, "GET", "/api/execution-scopes", json!(null)).await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(
		result["items"],
		json!([]),
		"Agents that remove file Tools must not gain working files"
	);
	cleanup(f, &url, &schema).await;
}

use aidash_server::{
	capabilities::{Profile, Runtime},
	federation::Federation,
};
use serde_json::Value;
use uuid::Uuid;

async fn fixture_blob(pool: &sqlx::PgPool, root: &std::path::Path, digest: &str, bytes: &[u8]) {
	use reinhardt::query::{Alias, OnConflict, PostgresQueryBuilder, Query};
	let tenant_hash = aidash_server::capabilities::objects::digest(b"acme");
	let path = root
		.join("tenants")
		.join(tenant_hash)
		.join("sha256")
		.join(&digest[..2])
		.join(&digest[2..4])
		.join(digest);
	tokio::fs::create_dir_all(path.parent().unwrap())
		.await
		.unwrap();
	tokio::fs::write(path, bytes).await.unwrap();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("core_blobs"))
			.columns(["tenant", "digest", "size"].map(Alias::new))
			.values(vec![
				"acme".into(),
				digest.into(),
				(bytes.len() as i64).into(),
			])
			.unwrap()
			.on_conflict(
				OnConflict::columns(["tenant", "digest"].map(Alias::new))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(pool)
	.await
	.unwrap();
}

#[derive(Clone)]
struct CoreFixture {
	_storage: Arc<tempfile::TempDir>,
	http_client: reqwest::Client,
	f: Federation,
	app: common::TestApplication,
	token: String,
	policy: Value,
	task: Uuid,
	url: String,
	schema: String,
	root: std::path::PathBuf,
}
impl CoreFixture {
	async fn close(self) {
		if let Some(runner) = &self.f.store.capabilities.0.runner {
			use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
			// Tear down only this fixture's persisted interpreter identities. The
			// real node guardian proves termination before the database is removed.
			let sessions: Vec<Value> = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("data"))
					.from(Alias::new("core_records"))
					.and_where(Expr::col(Alias::new("kind")).eq("python_session"))
					.and_where(
						Expr::col(Alias::new("state")).is_in(["initial", "frozen", "running"]),
					)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(self.f.store.pool.fixture_driver())
			.await
			.unwrap();

			for session in sessions {
				let sid: Uuid = serde_json::from_value(session["session_id"].clone()).unwrap();
				let reply: Value = self
					.http_client
					.post(format!(
						"{}/v1/sessions/{sid}/stop",
						runner.endpoint.trim_end_matches('/')
					))
					.bearer_auth(std::env::var(&runner.token_env).unwrap())
					.timeout(std::time::Duration::from_secs(60))
					.send()
					.await
					.unwrap()
					.error_for_status()
					.unwrap()
					.json()
					.await
					.unwrap();
				assert_eq!(
					reply["termination_confirmed"], true,
					"fixture interpreter must be stopped: {reply}"
				);
			}
		}
		cleanup(self.f, &self.url, &self.schema).await;
		let _ = tokio::fs::remove_dir_all(self.root).await;
	}
}
type CoreFuture = Shared<BoxFuture<'static, CoreFixture>>;
type EndpointFuture = Shared<BoxFuture<'static, String>>;

#[derive(Clone)]
enum FixtureRunner {
	Disabled,
	#[cfg(feature = "capability-runtime-tests")]
	Profile,
	Stub(EndpointFuture),
}

struct WorkerControl {
	stop: tokio::sync::watch::Sender<bool>,
	receiver: tokio::sync::watch::Receiver<bool>,
}

#[rstest::fixture]
fn worker_control() -> WorkerControl {
	let (stop, receiver) = tokio::sync::watch::channel(false);
	WorkerControl { stop, receiver }
}

struct CapabilityWorker {
	stop: tokio::sync::watch::Sender<bool>,
	handle: tokio::task::JoinHandle<aidash_server::Result<()>>,
}

impl std::future::Future for CapabilityWorker {
	type Output = Result<aidash_server::Result<()>, tokio::task::JoinError>;
	fn poll(
		mut self: std::pin::Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
	) -> std::task::Poll<Self::Output> {
		std::pin::Pin::new(&mut self.handle).poll(cx)
	}
}

impl Drop for CapabilityWorker {
	fn drop(&mut self) {
		let _ = self.stop.send(true);
		self.handle.abort();
	}
}

#[cfg(feature = "capability-runtime-tests")]
#[rstest::fixture]
async fn running_runtime(
	#[future] runtime_fixture: CoreFixture,
	worker_control: WorkerControl,
) -> (CoreFixture, aidash_server::domain::Run, CapabilityWorker) {
	let c = Box::pin(runtime_fixture).await;
	let run = admit(&c).await;
	let worker = CapabilityWorker {
		stop: worker_control.stop,
		handle: tokio::spawn(aidash_server::capabilities::operations::run(
			c.f.store.clone(),
			worker_control.receiver,
		)),
	};
	(c, run, worker)
}

#[rstest::fixture]
fn core_storage(
	#[from(reinhardt::test::fixtures::temp_dir)] directory: tempfile::TempDir,
) -> Arc<tempfile::TempDir> {
	Arc::new(directory)
}
#[rstest::fixture]
fn core_runtime(
	#[default("aidash://execution-test")] node: &str,
	core_storage: Arc<tempfile::TempDir>,
	#[default(FixtureRunner::Disabled)] runner: FixtureRunner,
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) -> common::RuntimeFuture {
	let node = aidash_domain::nodes::DeploymentId::parse(node).unwrap();
	async move {
		let mut owner = runtime.await;
		let f = &mut owner.federation;
		f.config.deployment = node.clone();
		f.store.deployment = node.clone();
		f.registry = aidash_server::registry::Registry::new(f.store.pool.clone(), &node)
			.unwrap()
			.with_keyless_origins(f.store.keyless.clone());
		let mut profile = match runner {
			#[cfg(feature = "capability-runtime-tests")]
			FixtureRunner::Profile => (*Runtime::from_env().unwrap().0).clone(),
			FixtureRunner::Disabled | FixtureRunner::Stub(_) => Profile {
				admission: true,
				outbound_origins: vec!["https://example.com".into()],
				storage: core_storage.path().to_owned(),
				..Profile::default()
			},
		};
		if let FixtureRunner::Stub(endpoint) = runner {
			profile.runner = Some(
				serde_json::from_value(json!({
					"endpoint":endpoint.await,"token_env":"AIDASH_COMMAND_CLEANUP_TEST_TOKEN",
					"image":"stub","runtime_class":"stub","namespace":"stub",
				}))
				.unwrap(),
			);
		}
		profile.storage = core_storage.path().to_owned();
		f.store.capabilities = Runtime::new(profile).unwrap();
		owner
	}
	.boxed()
	.shared()
}
#[rstest::fixture]
fn capability_fixture(
	#[default("aidash://execution-test")] _node: &str,
	#[default(async { "http://localhost:19999".into() }.boxed().shared())] endpoint: EndpointFuture,
	#[default(FixtureRunner::Disabled)] _runner: FixtureRunner,
	#[from(core_storage)] storage: Arc<tempfile::TempDir>,
	#[from(core_runtime)]
	#[with(_node,storage.clone(),_runner.clone())]
	_runtime: common::RuntimeFuture,
	#[from(common::native_application)]
	#[with(Default::default(),aidash_server::sse::Service::new(Default::default()),Arc::new(|router|router),_runtime.clone())]
	application: common::ApplicationFuture,
	#[from(reinhardt::test::fixtures::http_client)] http_client: reqwest::Client,
) -> CoreFuture {
	async move {let application=application.await;let (f,url,schema)=application.runtime.parts();let app=application.application;let root=storage.path().to_owned();let endpoint=endpoint.await;
let (mut policy, token, task) = bootstrap(&f, &app, &endpoint).await;
	policy["subjects"]
		[aidash_server::domain::qualified_agent(&aidash_domain::nodes::NodeAddress::derived(f.config.deployment.as_str(), "acme"), "research", "1.1.0")] =
		json!({"kind":"agent", "node": aidash_domain::nodes::NodeName::derived("acme")});
	policy["subjects"]["bob"] = json!({"kind":"user"});
	let (status, response) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy.clone()}),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	let mut entry = f.registry.get(&aidash_domain::tenancy::TenantName::new("acme").unwrap(), "research", "1.0.0").await.unwrap();
	entry.version = "1.1.0".into();
	entry.binding_normalization = None;
	entry.config["bindings"] = json!([]);
	entry.config["remove_default"] = json!([]);
	let offered: Vec<aidash_server::registry::Entry> = f
		.registry
		.list(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			&aidash_server::registry::Search {
				kind: Some("bundle".into()),
				..Default::default()
			},
		)
		.await
		.unwrap()
		.into_iter()
		.filter(|bundle| bundle.tags.iter().any(|tag| tag == "sandboxed"))
		.collect();
	let approvals: Vec<Value> = offered
		.iter()
		.flat_map(|bundle| {
			std::iter::once(json!({"id":bundle.id,"version":bundle.version})).chain(
				bundle.config["members"]
					.as_array()
					.unwrap()
					.iter()
					.map(|member| json!({"id":member["id"],"version":member["version"]})),
			)
		})
		.map(|entry| json!({"entry":entry,"enabled":true,"expected_revision":0,"scope":"node"}))
		.collect();
	let (status, approved) = request(
		&app,
		&token,
		"POST",
		"/api/authorization/acme/catalog/approvals",
		json!({"node":aidash_domain::nodes::NodeName::derived("acme"), "approvals": approvals }),
	)
	.await;
	assert_eq!(status, 200, "{approved}");
	entry.config["bindings"] = json!(offered.iter().map(|bundle| json!({"kind":"bundle","target":{"registry_deployment":f.config.deployment,"id":bundle.id,"version":bundle.version},"narrow":{}})).collect::<Vec<_>>());
	let instructions = "---\nname: analysis\ndescription: Analyze the selected CSV\nlicense: MIT\n---\nLoad this only when selected. Scripts are data.";
	let files = json!([{"path":"references/guide.md","content":"東京\n"},{"path":"scripts/analyze.py","content":"raise RuntimeError('must never run on activation')"}]);
	let digest =
		aidash_server::registry::digest(&json!({"instructions":instructions,"files":files}));
	let source = json!({"id":"fixture-skills","version":"1.0.0","kind":"source","name":{"en":"Fixture Skills"},"description":{"en":"Explicit immutable Skill attachments"},"schema":{},"config":{"schema_version":1,"source":{"adapter":"skill_attachments","attachments":[{"skill_id":Uuid::new_v4(),"origin":"fixture:project","digest":digest,"instructions":instructions,"files":files},{"skill_id":Uuid::new_v4(),"origin":"fixture:user","digest":digest,"instructions":instructions,"files":files}]}}});
	let (status, response) =
		request(&app, &token, "POST", "/api/registry", source).await;
	assert_eq!(status, 200, "{response}");
	let (status, response) = request(
		&app,
		&token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"fixture-skills","version":"1.0.0"},"expected_revision":0,"enabled":true,"scope":"node"}),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	entry.config["bindings"].as_array_mut().unwrap().push(json!({"kind":"source","target":{"registry_deployment":f.config.deployment,"id":"fixture-skills","version":"1.0.0"},"narrow":{}}));

	let (status, response) = request(
		&app,
		&token,
		"POST",
		"/api/registry",
		json!(entry),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	let (status, response) = request(
		&app,
		&token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"research","version":"1.1.0"},"expected_revision":0,"enabled":true,"carry_over":false,"scope":"node"}),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	// Keep the shared disposable service fixture alive for the lifetime of this fixture.
	// The pool alone does not own the Compose services.

	CoreFixture {
		_storage: storage,
http_client,
		f,
		app,
		token,
		policy,
		task,
		url,
		schema,
		root,
	}
} .boxed().shared()
}

#[rstest::rstest]
#[tokio::test]
async fn shell_never_falls_back_to_the_host(#[future] capability_fixture: CoreFixture) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let (status, result) = request(&c.app, &c.token, "POST", &format!("/api/executions/{}/shell", run.id), json!({
		"idempotency_key":Uuid::new_v4(),"command":"printf unsafe > MUST_NOT_EXIST", "expected_revision":1
	})).await;
	assert!(matches!(status, 400 | 403 | 409), "{result}");
	if status == 400 {
		assert_eq!(
			result["error"]["message"],
			"Run has no bound operation: shell"
		);
	}
	if status == 409 {
		assert!(
			result["error"]["code"]
				.as_str()
				.unwrap()
				.contains("RUNTIME_UNAVAILABLE")
		);
	}
	c.close().await;
}

#[cfg(feature = "capability-runtime-tests")]
#[rstest::fixture]
async fn runtime_fixture(
	#[from(capability_fixture)]
	#[with("aidash://execution-test", async { "http://localhost:19999".into() }.boxed().shared(), FixtureRunner::Profile)]
	core: CoreFuture,
) -> CoreFixture {
	let c = core.await;
	assert!(
		c.f.store.capabilities.0.runner.is_some(),
		"set a verified isolated runner profile"
	);
	c
}

#[cfg(feature = "capability-runtime-tests")]
async fn operation_until(
	c: &CoreFixture,
	run: Uuid,
	kind: &str,
	operation: &Value,
	wanted: &[&str],
) -> Value {
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
	loop {
		let (status, result) = request(
			&c.app,
			&c.token,
			"POST",
			&format!("/api/executions/{run}/{kind}/poll"),
			json!({"operation_id":operation}),
		)
		.await;
		assert_eq!(status, 200, "{result}");
		if wanted.contains(&result["status"].as_str().unwrap()) {
			return result;
		}
		assert!(
			!matches!(
				result["status"].as_str(),
				Some("failed" | "uncertain" | "withdrawn")
			),
			"unexpected terminal result: {result}"
		);
		assert!(
			tokio::time::Instant::now() < deadline,
			"operation did not reach {wanted:?}: {result}"
		);
		tokio::time::sleep(std::time::Duration::from_millis(200)).await;
	}
}

#[cfg(feature = "capability-runtime-tests")]
#[rstest::rstest]
#[tokio::test]
async fn isolated_shell_survives_worker_restart_and_confirms_descendant_cancellation(
	#[future] runtime_fixture: CoreFixture,
) {
	let c = Box::pin(runtime_fixture).await;
	let run = admit(&c).await;
	let (stopping, receiver) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash_server::capabilities::operations::run(
		c.f.store.clone(),
		receiver.clone(),
	));
	let path = format!("/api/executions/{}/shell", run.id);
	let input = json!({"idempotency_key":Uuid::new_v4(),"command":"printf '一度だけ\\n' >> count.txt; sleep 5; printf 'finished\\n'", "expected_revision":1});
	let (status, first) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{first}");
	assert_eq!(first["status"], "prepared");
	let (status, busy) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/executions/{}/files/search", run.id),
		json!({"query":"x","mode":"literal","scope":"working"}),
	)
	.await;
	assert_eq!(status, 409, "{busy}");
	operation_until(&c, run.id, "shell", &first["operation_id"], &["running"]).await;
	worker.abort();
	let _ = worker.await;
	let worker = tokio::spawn(aidash_server::capabilities::operations::run(
		c.f.store.clone(),
		receiver,
	));
	let (status, repeated) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{repeated}");
	assert_eq!(repeated["operation_id"], first["operation_id"]);
	let mut changed = input;
	changed["command"] = json!("printf duplicate >> count.txt");
	assert_eq!(
		request(&c.app, &c.token, "POST", &path, changed).await.0,
		409
	);
	let finished =
		operation_until(&c, run.id, "shell", &first["operation_id"], &["completed"]).await;
	assert_eq!(finished["termination_confirmed"], true);
	assert!(finished["output"].as_str().unwrap().contains("finished"));
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/executions/{}/execution-scope", run.id),
		Value::Null,
	)
	.await;
	let (status, read) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/executions/{}/files/read", run.id),
		json!({"path":format!("threads/{}/count.txt",run.thread_id),"representation":"text"}),
	)
	.await;
	assert_eq!(status, 200, "{read}");
	assert_eq!(read["content"], "一度だけ\n");
	let (status, long) = request(&c.app,&c.token,"POST",&path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"command":"python -c 'import os,signal,time; os.setsid(); signal.signal(signal.SIGTERM,signal.SIG_IGN); time.sleep(60)' & wait"})).await;
	assert_eq!(status, 200, "{long}");
	operation_until(&c, run.id, "shell", &long["operation_id"], &["running"]).await;
	let (status, cancel) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/executions/{}/shell/cancel", run.id),
		json!({"operation_id":long["operation_id"]}),
	)
	.await;
	assert_eq!(status, 200, "{cancel}");
	let cancelled =
		operation_until(&c, run.id, "shell", &long["operation_id"], &["cancelled"]).await;
	assert_eq!(cancelled["termination_confirmed"], true);
	assert_eq!(cancelled["effects_may_have_occurred"], true);
	stopping.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}
#[cfg(feature = "capability-runtime-tests")]
#[rstest::rstest]
#[tokio::test]
async fn isolated_python_preserves_heap_and_requires_reset_acknowledgement(
	#[future] runtime_fixture: CoreFixture,
) {
	let c = Box::pin(runtime_fixture).await;
	let run = admit(&c).await;
	let (stop, receiver) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash_server::capabilities::operations::run(
		c.f.store.clone(),
		receiver,
	));
	let path = format!("/api/executions/{}/python", run.id);
	let input = json!({"idempotency_key":Uuid::new_v4(),"code":"counter = 41\nfrom pathlib import Path\nPath('value.txt').write_text('東京')\nprint(counter)","expected_revision":1});
	let (status, first) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{first}");
	let done = operation_until(&c, run.id, "python", &first["operation_id"], &["completed"]).await;
	assert_eq!(done["writer_frozen"], true, "{done}");
	assert!(done["output"].as_str().unwrap().contains("41"));
	let (status, duplicate) = request(&c.app, &c.token, "POST", &path, input).await;
	assert_eq!(status, 200, "{duplicate}");
	assert_eq!(duplicate["operation_id"], first["operation_id"]);
	let input = json!({"idempotency_key":Uuid::new_v4(),"code":"counter += 1\nprint(counter)","expected_revision":done["revision"],"expected_session_id":done["session_id"]});
	let (status, next) = request(&c.app, &c.token, "POST", &path, input).await;
	assert_eq!(status, 200, "{next}");
	let second = operation_until(&c, run.id, "python", &next["operation_id"], &["completed"]).await;
	assert_eq!(second["session_id"], done["session_id"]);
	assert!(second["output"].as_str().unwrap().contains("42"));
	let (status,patched)=request(&c.app,&c.token,"POST",&format!("/api/executions/{}/patch",run.id),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":second["revision"],"patch":"*** Begin Patch\n*** Add File: changed.txt\n+changed outside Python\n*** End Patch","preconditions":{"changed.txt":null}})).await;
	assert_eq!(status, 200, "{patched}");
	let mut input = json!({"idempotency_key":Uuid::new_v4(),"code":"print(counter)","expected_revision":patched["revision"],"expected_session_id":second["session_id"]});
	let (status, reset) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{reset}");
	assert_eq!(reset["error"]["code"], "SESSION_RESET");
	assert_ne!(reset["session_id"], second["session_id"]);
	input["expected_session_id"] = reset["session_id"].clone();
	let (status, ack) = request(&c.app, &c.token, "POST", &path, input).await;
	assert_eq!(status, 200, "{ack}");
	let failed = operation_until(&c, run.id, "python", &ack["operation_id"], &["failed"]).await;
	assert!(
		failed["error"]["details"]["type"] == "NameError",
		"{failed}"
	);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}
async fn admit(c: &CoreFixture) -> aidash_server::domain::Run {
	let (status, response) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/tasks/{}/delegate", c.task),
		json!({"node_id":c.f.config.deployment,"agent":{"id":"research","version":"1.1.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	c.f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0)
}
async fn source(c: &CoreFixture, thread: Uuid, text: &str) -> Uuid {
	let (status, response) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/threads/{thread}/channel-messages"),
		json!({"content":text,"idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(status, 200, "{response}");
	serde_json::from_value(response["message"]["id"].clone()).unwrap()
}
#[rstest::rstest]
#[tokio::test]
async fn only_the_requester_can_steer_with_an_idempotent_request(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let (status, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/executions/{}/execution-scope", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	let steer_path = format!(
		"/api/execution-scopes/{}/steer",
		area["id"].as_str().unwrap()
	);
	let (status, result) = request(&c.app, &c.token, "POST", &steer_path, json!({"idempotency_key":Uuid::new_v4(),"expected_run_id":Uuid::new_v4(),"content":"Must only reach the selected Run"})).await;
	assert_eq!(status, 409, "{result}");
	let steer = json!({"idempotency_key":Uuid::new_v4(),"expected_run_id":run.id,"content":"Prioritize the current question"});
	let (_, bob) = async {
		(
			200_u16,
			common::issued_credential(&c.f.store, "acme", "bob").await,
		)
	}
	.await;
	assert_eq!(
		request(
			&c.app,
			bob["token"].as_str().unwrap(),
			"POST",
			&steer_path,
			steer.clone()
		)
		.await
		.0,
		404
	);
	assert!(
		c.f.store
			.run_inputs(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.is_empty(),
		"another requester cannot steer a privileged Run"
	);
	let (status, result) = request(&c.app, &c.token, "POST", &steer_path, steer.clone()).await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(
		request(&c.app, &c.token, "POST", &steer_path, steer)
			.await
			.0,
		200
	);
	let inputs = c.f.store.run_inputs(&TEST_FIXTURE, run.id).await.unwrap();
	assert_eq!(inputs.len(), 1);
	assert_eq!(inputs[0].content, "Prioritize the current question");
	c.close().await;
}

fn path_for_patch(id: Uuid) -> String {
	format!("/api/executions/{id}/patch")
}

#[rstest::rstest]
#[tokio::test]
async fn direct_skills_are_pinned_and_loaded_progressively(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let base = format!("/api/executions/{}/skills", run.id);
	let (status, list) =
		request(&c.app, &c.token, "POST", &format!("{base}/list"), json!({})).await;
	assert_eq!(status, 200, "{list}");
	let skills = list["skills"].as_array().unwrap();
	assert_eq!(skills.len(), 2);
	assert_eq!(skills[0]["name"], skills[1]["name"]);
	assert_ne!(skills[0]["skill_id"], skills[1]["skill_id"]);
	assert_ne!(skills[0]["origin"], skills[1]["origin"]);
	assert!(!list.to_string().contains("Load this only"));
	let skill = &skills[0];
	let (status, loaded) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("{base}/load"),
		json!({"skill_id":skill["skill_id"],"expected_digest":skill["digest"]}),
	)
	.await;
	assert_eq!(status, 200, "{loaded}");
	assert!(
		loaded["content"]
			.as_str()
			.unwrap()
			.contains("Load this only")
	);
	assert_eq!(loaded["skill"]["license"], "MIT");
	let (status,read)=request(&c.app,&c.token,"POST",&format!("{base}/read"),json!({"skill_id":skill["skill_id"],"digest":skill["digest"],"path":"references/guide.md","max_chars":1})).await;
	assert_eq!(status, 200, "{read}");
	assert_eq!(read["content"], "東");
	assert_eq!(read["next_offset"], 1);
	for (path, digest, expected) in [
		("references/guide.md", "stale", 409),
		(
			"../scripts/analyze.py",
			skill["digest"].as_str().unwrap(),
			400,
		),
	] {
		let (status, result) = request(
			&c.app,
			&c.token,
			"POST",
			&format!("{base}/read"),
			json!({"skill_id":skill["skill_id"],"digest":digest,"path":path}),
		)
		.await;
		assert_eq!(status, expected, "{result}");
	}
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/executions/{}/execution-scope", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(area["state"], "active");
	assert!(area.get("manifest").is_none());
	assert_eq!(area["epoch"], 1);
	c.close().await;
}

#[rstest::fixture]
async fn approval_fixture(
	#[future] capability_fixture: CoreFixture,
) -> (CoreFixture, aidash_server::domain::Run) {
	configure_approvals(Box::pin(capability_fixture).await).await
}
async fn configure_approvals(mut c: CoreFixture) -> (CoreFixture, aidash_server::domain::Run) {
	let run = admit(&c).await;
	c.policy["policies"][0]["resources"]["kinds"] = json!([
		"thread",
		"task",
		"run",
		"tool",
		"installation",
		"registry_entry",
		"bundle",
		"source",
		"message",
		"agent",
		"model",
		"skill",
		"memory",
		"working_area",
		"artifact",
		"human_request"
	]);
	// A dormant default still requires current Registry metadata visibility.
	c.policy["policies"].as_array_mut().unwrap().push(json!({"id":"default-decider-access","effect":"allow","subjects":{"any":true},"actions":["registry.read","decision.invoke"],"resources":{"kinds":["decider"]}}));
	c.policy["policies"].as_array_mut().unwrap().push(json!({"id":"approvable-outbound","effect":"allow","subjects":{"any":true},"actions":["capability.request","capability.approve"],"resources":{"kinds":["outbound"]}}));
	common::replace_policy(&c.f.store, "acme", c.policy.clone()).await;
	(c, run)
}
#[rstest::rstest]
#[tokio::test]
async fn approvals_bind_invocations_and_never_override_upper_denials(
	#[future] approval_fixture: (CoreFixture, aidash_server::domain::Run),
) {
	let (c, run) = Box::pin(approval_fixture).await;
	let path = format!("/api/executions/{}/outbound", run.id);
	let input = json!({"idempotency_key":Uuid::new_v4(),"url":"https://example.com/a"});
	let (status, pending) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{pending}");
	assert_eq!(pending["status"], "approval_required");
	assert!(pending["decided_by"].is_null(), "{pending}");
	let decision_path = format!(
		"/api/capabilities/approvals/{}/decide",
		pending["approval_id"].as_str().unwrap()
	);
	let (_, bob) = async {
		(
			200_u16,
			common::issued_credential(&c.f.store, "acme", "bob").await,
		)
	}
	.await;
	let decision = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1});
	assert_eq!(
		request(
			&c.app,
			bob["token"].as_str().unwrap(),
			"POST",
			&decision_path,
			decision.clone()
		)
		.await
		.0,
		404
	);
	let (status, allowed) =
		request(&c.app, &c.token, "POST", &decision_path, decision.clone()).await;
	assert_eq!(status, 200, "{allowed}");
	assert_eq!(allowed["state"], "approved");
	assert!(allowed["grant_id"].is_null());
	assert_eq!(
		request(&c.app, &c.token, "POST", &decision_path, decision).await,
		(200, allowed.clone())
	);
	assert_eq!(request(&c.app,&c.token,"POST",&decision_path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"choice":"allow_run","targets":["https://example.com"],"expires_at":chrono::Utc::now()+chrono::Duration::minutes(5)})).await.0,409);
	let (_, second) = request(
		&c.app,
		&c.token,
		"POST",
		&path,
		json!({"idempotency_key":Uuid::new_v4(),"url":"https://example.com/b"}),
	)
	.await;
	assert_eq!(second["status"], "approval_required");
	let (status, revoked) = request(
		&c.app,
		&c.token,
		"POST",
		&format!(
			"/api/capabilities/approvals/{}/revoke",
			pending["approval_id"].as_str().unwrap()
		),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":2}),
	)
	.await;
	assert_eq!(status, 200, "{revoked}");
	let (_, again) = request(&c.app, &c.token, "POST", &path, input).await;
	assert_eq!(again["status"], "blocked");
	let mut tightened = c.policy.clone();
	tightened["policies"].as_array_mut().unwrap().push(json!({"id":"upper-network-deny","effect":"deny","subjects":{"any":true},"actions":["network.get"],"resources":{"kinds":["outbound"]}}));
	common::replace_policy(&c.f.store, "acme", tightened.clone()).await;
	let (status, value) = request(
		&c.app,
		&c.token,
		"POST",
		&path,
		json!({"idempotency_key":Uuid::new_v4(),"url":"https://example.com/c"}),
	)
	.await;
	assert_eq!(status, 403, "{value}");
	c.close().await;
}
#[rstest::rstest]
#[tokio::test]
async fn withdrawing_the_agents_approval_stops_an_approved_request_before_it_is_sent(
	#[future] approval_fixture: (CoreFixture, aidash_server::domain::Run),
	worker_control: WorkerControl,
) {
	let (c, run) = Box::pin(approval_fixture).await;
	// The person approves the fetch, and no worker has started it yet.
	let (status, pending) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/executions/{}/outbound", run.id),
		json!({"idempotency_key":Uuid::new_v4(),"url":"https://example.com/a"}),
	)
	.await;
	assert_eq!(status, 200, "{pending}");
	let approval = pending["approval_id"].as_str().unwrap();
	let (status, allowed) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/capabilities/approvals/{approval}/decide"),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1}),
	)
	.await;
	assert_eq!(status, 200, "{allowed}");
	// The record is read from the database: once the Agent is withdrawn the Run's own routes
	// answer `403`, so they cannot report it.
	let record = async || -> (String, Value) {
		sqlx::query_as("SELECT state, data FROM core_records WHERE id = $1 AND kind = 'outbound'")
			.bind(approval.parse::<Uuid>().unwrap())
			.fetch_one(c.f.store.pool.fixture_driver())
			.await
			.unwrap()
	};
	assert_eq!(record().await.0, "approved");
	// The Agent's Catalog Approval goes while the approval to fetch stays: only a Run that may
	// still use its Agent acts (the Use Recheck, #769).
	let authorization = aidash_server::authorization::Authorization {
		pool: c.f.store.pool.clone(),
	};
	let revision = authorization
		.catalog("acme", &run.node)
		.await
		.unwrap()
		.into_iter()
		.find(|binding| {
			binding.entry_id == run.agent_id && binding.entry_version == run.agent_version
		})
		.unwrap()
		.revision;
	authorization
		.set_catalog(
			"acme",
			&run.node,
			&aidash_server::registry::EntityRef {
				id: run.agent_id.clone(),
				version: run.agent_version.clone(),
			},
			revision,
			false,
			"deployment_administrator",
		)
		.await
		.unwrap();
	let WorkerControl { stop, receiver } = worker_control;
	let worker = tokio::spawn(aidash_server::capabilities::operations::run(
		c.f.store.clone(),
		receiver,
	));
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
	let (state, data) = loop {
		let (state, data) = record().await;
		if state != "approved" {
			break (state, data);
		}
		assert!(tokio::time::Instant::now() < deadline, "never picked up");
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	};
	// It was refused at the start, not sent: nothing was attempted and nothing was stored.
	assert_eq!(state, "uncertain", "{data}");
	assert_eq!(data["error"]["code"], "AUTHORITY_WITHDRAWN", "{data}");
	assert!(data.get("final_url").is_none(), "{data}");
	assert!(data.get("output_file").is_none(), "{data}");
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}
#[rstest::rstest]
#[tokio::test]
async fn reusable_grants_require_explicit_scope_and_revocation_withdraws_reuse(
	#[future] approval_fixture: (CoreFixture, aidash_server::domain::Run),
) {
	let (c, run) = Box::pin(approval_fixture).await;
	let path = format!("/api/executions/{}/outbound", run.id);
	let (_, pending) = request(
		&c.app,
		&c.token,
		"POST",
		&path,
		json!({"idempotency_key":Uuid::new_v4(),"url":"https://example.com/a"}),
	)
	.await;
	let decision_path = format!(
		"/api/capabilities/approvals/{}/decide",
		pending["approval_id"].as_str().unwrap()
	);
	assert_eq!(
		request(
			&c.app,
			&c.token,
			"POST",
			&decision_path,
			json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"choice":"allow_run"})
		)
		.await
		.0,
		400
	);
	let (status,grant)=request(&c.app,&c.token,"POST",&decision_path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"choice":"allow_run","targets":["https://example.com"],"expires_at":chrono::Utc::now()+chrono::Duration::minutes(5)})).await;
	assert_eq!(status, 200, "{grant}");
	let (_, reused) = request(
		&c.app,
		&c.token,
		"POST",
		&path,
		json!({"idempotency_key":Uuid::new_v4(),"url":"https://example.com/b"}),
	)
	.await;
	assert_eq!(reused["status"], "running");
	assert_eq!(reused["grant_id"], grant["grant_id"]);
	let (status, value) = request(
		&c.app,
		&c.token,
		"POST",
		&format!(
			"/api/capabilities/grants/{}/revoke",
			grant["grant_id"].as_str().unwrap()
		),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1}),
	)
	.await;
	assert_eq!(status, 200, "{value}");
	let (_, pending) = request(
		&c.app,
		&c.token,
		"POST",
		&path,
		json!({"idempotency_key":Uuid::new_v4(),"url":"https://example.com/c"}),
	)
	.await;
	assert_eq!(pending["status"], "approval_required");
	c.close().await;
}
#[path = "core_capabilities/lifecycle.rs"]
mod lifecycle_tests;

#[path = "core_capabilities/area_queue.rs"]
mod area_queue_tests;

#[path = "core_capabilities/duties.rs"]
mod duty_tests;

#[path = "core_capabilities/agent_nodes.rs"]
mod agent_node_tests;

#[cfg(feature = "capability-runtime-tests")]
#[path = "core_capabilities/runtime_faults.rs"]
mod runtime_fault_tests;

#[path = "core_capabilities/configuration.rs"]
mod configuration_tests;

#[cfg(feature = "capability-runtime-tests")]
#[path = "core_capabilities/reference_limits.rs"]
mod reference_limit_tests;

#[cfg(feature = "capability-runtime-tests")]
#[path = "core_capabilities/network.rs"]
mod network_tests;

#[path = "core_capabilities/session_boundaries.rs"]
mod session_boundary_tests;

#[path = "core_capabilities/previews.rs"]
mod preview_tests;

#[path = "core_capabilities/scratch_patch.rs"]
mod scratch_patch;

#[path = "core_capabilities/live_previews.rs"]
mod live_preview_tests;

#[cfg(feature = "capability-runtime-tests")]
#[path = "core_capabilities/extraction_lifecycle.rs"]
mod extraction_lifecycle_tests;

#[path = "core_capabilities/subject_sessions.rs"]
mod subject_session_tests;

#[path = "core_capabilities/upload_bounds.rs"]
mod upload_bound_tests;

#[cfg(feature = "capability-runtime-tests")]
#[path = "core_capabilities/reference_dispatch.rs"]
mod reference_dispatch_tests;

#[cfg(feature = "capability-runtime-tests")]
#[path = "core_capabilities/publication_storage.rs"]
mod publication_storage_tests;

#[path = "core_capabilities/upload_scope.rs"]
mod upload_scope_tests;

#[path = "core_capabilities/quota_dedup.rs"]
mod quota_dedup_tests;

#[path = "core_capabilities/torn_intents.rs"]
mod torn_intents_tests;

#[path = "core_capabilities/reclamation_lock.rs"]
mod reclamation_lock_tests;

#[path = "core_capabilities/review_regressions.rs"]
mod review_regression_tests;

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

use reinhardt::query::SimpleExpr;

#[rstest::fixture]
fn provider_endpoint(
	#[default(async { panic!("provider endpoint must be injected") }.boxed().shared())]
	provider: upstream_fixtures::UpstreamFuture,
) -> EndpointFuture {
	async move { provider.await.url.clone() }.boxed().shared()
}
#[path = "core_capabilities/deferred_exposure.rs"]
mod deferred_exposure_tests;

#[path = "core_capabilities/completion_gate.rs"]
mod completion_gate_tests;

#[path = "core_capabilities/journal_query.rs"]
mod journal_query_tests;

#[path = "core_capabilities/reference_set.rs"]
mod reference_set_tests;
