use super::*;
use aidash_domain::{
	RunControl, RunMetadata, RunPhase,
	capabilities::{
		CoreCapabilities,
		errors::COMMAND_TIMED_OUT,
		operations::reconciliation::{Area, Limits},
	},
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::sync::{Arc, Mutex};
struct Trace {
	operation: Snapshot,
	area: Area,
	run: RunMetadata,
	limits: Limits,
	timeout_ceiling: Option<u64>,
	capabilities: CoreCapabilities,
	health: Value,
	observed: Value,
	download: Value,
	files: Vec<MountedFile>,
	chunks: Vec<Vec<u8>>,
	calls: Vec<String>,
	saved: Vec<Snapshot>,
	events: Vec<Value>,
	published: Vec<MountedFile>,
	commit: bool,
	failure: Option<(String, bool)>,
	/// Request bodies sent to the runner.
	bodies: Vec<Value>,
	reusable: Option<Uuid>,
}
#[derive(Clone)]
struct Repository(Arc<Mutex<Trace>>);
struct Scope(Repository);
#[fixture]
fn repository() -> Repository {
	Repository(Arc::new(Mutex::new(Trace {
		operation: Snapshot {
			id: Uuid::from_u128(1),
			area_id: Uuid::from_u128(2),
			run_id: Uuid::from_u128(3),
			tenant: "tenant".into(),
			principal: "owner".into(),
			credential_id: Uuid::from_u128(4),
			subjects: json!(["owner"]),
			digest: "digest".into(),
			kind: "shell".into(),
			state: "submitted".into(),
			epoch: 5,
			generation: 6,
			revision: 7,
			policy_revision: 8,
			input: json!({"command":"echo fixture","seconds":1}),
			result: json!({}),
			runner_instance: Some("runner".into()),
		},
		area: Area {
			id: Uuid::from_u128(2),
			workspace_id: Uuid::from_u128(10),
			generation: 6,
			revision: 7,
			epoch: 5,
			manifest: json!([]),
		},
		run: RunMetadata {
			node: "brave-otter-k3m9q2xa".parse().unwrap(),
			id: Uuid::from_u128(3),
			task_id: Uuid::from_u128(9),
			workspace_id: Uuid::from_u128(10),
			home_node: "aidash://local".into(),
			agent_id: "agent".into(),
			agent_version: "1".into(),
			phase: RunPhase::Ready,
			control: RunControl::Active,
			step: 0,
			revision: 1,
			observed_input_seq: 0,
			ledger_worker_ready: true,
			control_revision: 0,
			control_source: Default::default(),
			error: None,
			lease_owner: None,
			lease_until: None,
			updated_at: chrono::Utc::now(),
		},
		limits: Limits {
			admission: true,
			working_bytes: 1024,
			output_bytes: 1024,
			read_bytes: 8,
			maximum_seconds: 600,
		},
		timeout_ceiling: None,
		capabilities: CoreCapabilities {
			shell: true,
			python: true,
			..Default::default()
		},
		health: json!({"instance":"runner"}),
		observed: json!({"status":"running","stdout":""}),
		download: Value::Null,
		files: vec![],
		chunks: vec![],
		calls: vec![],
		saved: vec![],
		events: vec![],
		published: vec![],
		commit: false,
		failure: None,
		bodies: vec![],
		reusable: None,
	})))
}
impl Repository {
	fn call(&self, name: &str) -> Result<()> {
		let mut trace = self.0.lock().unwrap();
		trace.calls.push(name.into());
		if let Some((boundary, forbidden)) = &trace.failure
			&& boundary == name
		{
			if *forbidden {
				Err(Error::Forbidden)
			} else {
				Err(Error::External(format!("fault:{name}")))
			}
		} else {
			Ok(())
		}
	}
}
impl Drop for Scope {
	fn drop(&mut self) {
		self.0.0.lock().unwrap().calls.push("drop".into());
	}
}
#[async_trait]
impl OperationReconciliationRepository for Repository {
	async fn snapshot(&self, id: Uuid) -> Result<Snapshot> {
		assert_eq!(id, Uuid::from_u128(1));
		self.call("snapshot")?;
		Ok(self.0.lock().unwrap().operation.clone())
	}
	async fn begin(&self, _: &Snapshot) -> Result<Box<dyn OperationReconciliationScope + '_>> {
		self.call("begin")?;
		Ok(Box::new(Scope(self.clone())))
	}
	async fn withdraw(&self, snapshot: &Snapshot) -> Result<()> {
		assert_eq!(snapshot.id, Uuid::from_u128(1));
		assert_eq!(snapshot.area_id, Uuid::from_u128(2));
		self.call("withdraw")
	}
}
#[async_trait]
impl OperationReconciliationScope for Scope {
	fn limits(&self) -> Limits {
		self.0.0.lock().unwrap().limits
	}
	fn timeout_ceiling(&self, _: &str) -> Option<u64> {
		self.0.0.lock().unwrap().timeout_ceiling
	}
	async fn load(&mut self, _: Uuid, run: Uuid) -> Result<Loaded> {
		assert_eq!(run, Uuid::from_u128(3));
		self.0.call("load")?;
		let trace = self.0.0.lock().unwrap();
		Ok(Loaded {
			run: trace.run.clone(),
			area: trace.area.clone(),
			operation: trace.operation.clone(),
		})
	}
	async fn configuration(&mut self) -> Result<CoreCapabilities> {
		self.0.call("configuration")?;
		Ok(self.0.0.lock().unwrap().capabilities.clone())
	}
	async fn require_builtin(&mut self, kind: &str) -> Result<()> {
		self.0.call(&format!("require:{kind}"))
	}
	async fn authorize_packages(&mut self, _: &Value) -> Result<()> {
		self.0.call("packages")
	}
	async fn set_area(&mut self, state: &str) -> Result<()> {
		self.0.call(&format!("area:{state}"))
	}
	async fn complete_python(&mut self, _: &Snapshot, observed: &Value) -> Result<()> {
		self.0
			.call(&format!("python:{}", observed["writer_frozen"]))
	}
	async fn persist(&mut self, operation: &Snapshot) -> Result<()> {
		self.0.call("persist")?;
		self.0.0.lock().unwrap().saved.push(operation.clone());
		Ok(())
	}
	async fn health(&mut self, cancelling: bool, python: bool) -> Result<Value> {
		self.0.call(&format!("health:{cancelling}:{python}"))?;
		Ok(self.0.0.lock().unwrap().health.clone())
	}
	async fn request(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
		self.0.call(&format!("request:{method}:{path}"))?;
		// Tests that set no download read the `abc` output chunk.
		if method == "GET"
			&& path.contains("/files/")
			&& self.0.0.lock().unwrap().download.is_null()
		{
			return Ok(
				json!({"data":STANDARD.encode("abc"),"offset":0,"size":3,"digest":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"}),
			);
		}
		if path == "/v1/operations" {
			assert!(body.as_ref().unwrap()["operation_id"] == json!(Uuid::from_u128(1)));
		}
		let mut trace = self.0.0.lock().unwrap();
		trace.bodies.extend(body);
		if path.contains("/files/") {
			return Ok(trace.download.clone());
		}
		Ok(trace.observed.clone())
	}
	async fn dispatch_files(&mut self, _: &Snapshot) -> Result<Vec<MountedFile>> {
		self.0.call("dispatch_files")?;
		Ok(self.0.0.lock().unwrap().files.clone())
	}
	async fn upload_files(&mut self, _: &Snapshot) -> Result<Vec<MountedFile>> {
		self.0.call("upload_files")?;
		Ok(self.0.0.lock().unwrap().files.clone())
	}
	async fn read_chunk(&mut self, _: &MountedFile, _: u64) -> Result<Vec<u8>> {
		self.0.call("read_chunk")?;
		let mut trace = self.0.0.lock().unwrap();
		Ok(if trace.chunks.is_empty() {
			vec![]
		} else {
			trace.chunks.remove(0)
		})
	}
	async fn verified(&mut self, _: &MountedFile) -> Result<()> {
		self.0.call("verified")
	}
	async fn reuse_output(&mut self, digest: &str, size: u64) -> Result<Option<Uuid>> {
		self.0.call("reuse_output")?;
		assert_eq!(
			digest,
			"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
		);
		assert_eq!(size, 3);
		Ok(self.0.0.lock().unwrap().reusable)
	}
	async fn begin_output(&mut self, _: u64) -> Result<()> {
		self.0.call("begin_output")
	}
	async fn write_output(&mut self, _: &[u8]) -> Result<()> {
		self.0.call("write_output")
	}
	async fn finish_output(&mut self, expected: &str) -> Result<(Uuid, String)> {
		self.0.call("finish_output")?;
		Ok((Uuid::from_u128(11), expected.into()))
	}
	async fn put(&mut self, kind: &str, bytes: &[u8]) -> Result<(Uuid, String)> {
		self.0.call(&format!("put:{kind}"))?;
		if kind == "output" {
			assert_eq!(bytes, b"stdout");
		}
		Ok((Uuid::from_u128(12), "saved-digest".into()))
	}
	async fn publish(&mut self, entries: Vec<MountedFile>) -> Result<i64> {
		self.0.call("publish")?;
		let mut trace = self.0.0.lock().unwrap();
		trace.published = entries;
		Ok(trace.area.revision + 1)
	}
	async fn event(&mut self, data: Value) -> Result<()> {
		self.0.call("event")?;
		self.0.0.lock().unwrap().events.push(data);
		Ok(())
	}
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()> {
		self.0.call("finish")?;
		self.0.0.lock().unwrap().commit = result.is_ok();
		result
	}
}

#[rstest]
#[case::image("output", b"\x89PNG\r\n\x1a\nbody".as_slice(), "image/png")]
#[case::spoofed_extension("spoof.png", b"not an image".as_slice(), "application/octet-stream")]
#[tokio::test]
async fn exported_file_media_type_follows_verified_image_signature(
	repository: Repository,
	#[case] path: &str,
	#[case] bytes: &[u8],
	#[case] expected: &str,
) {
	let object = "a".repeat(64);
	// Output digests are validated as lowercase SHA-256 hex.
	let digest = "d".repeat(64);
	{
		let mut trace = repository.0.lock().unwrap();
		trace.observed = json!({"status":"completed","termination_confirmed":true,"executed":true,"files":[{"path":path,"object_id":object,"digest":digest,"size":bytes.len()}],"stdout":STANDARD.encode("stdout"),"displays":[],"exit_code":0});
		trace.download =
			json!({"data":STANDARD.encode(bytes),"offset":0,"size":bytes.len(),"digest":digest});
	}
	run(&repository).await.unwrap();
	assert_eq!(
		repository.0.lock().unwrap().published[0].media_type,
		expected
	);
}
async fn run(repository: &Repository) -> Result<()> {
	drive(repository, Uuid::from_u128(1)).await
}
#[rstest]
#[tokio::test]
async fn a_prepared_operation_journals_submission_before_dispatching_to_another_process(
	repository: Repository,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.state = "prepared".into();
		trace.operation.runner_instance = None;
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(
		trace.calls,
		vec![
			"snapshot",
			"begin",
			"load",
			"configuration",
			"require:shell",
			"health:false:false",
			"persist",
			"persist",
			"finish",
			"drop"
		]
	);
	assert_eq!(trace.saved.len(), 2);
	assert_eq!(trace.saved[0].state, "submitted");
	assert_eq!(trace.saved[0].runner_instance.as_deref(), Some("runner"));
	assert_eq!(trace.saved[1].result, json!({"dispatch_pending":true}));
	assert!(trace.events.is_empty());
	assert!(trace.commit);
}
#[rstest]
#[tokio::test]
async fn a_prepared_cancelled_operation_never_contacts_the_runner(repository: Repository) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.state = "prepared".into();
		trace.run.control = RunControl::Cancelled;
		trace.capabilities.shell = false;
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(
		trace.calls,
		vec![
			"snapshot",
			"begin",
			"load",
			"area:active",
			"persist",
			"finish",
			"drop"
		]
	);
	assert_eq!(trace.saved[0].state, "cancelled");
	assert_eq!(
		trace.saved[0].result,
		json!({"termination_confirmed":true,"effects_may_have_occurred":false})
	);
}
#[rstest]
#[case::begin("begin")]
#[case::current_run("load")]
#[case::configuration("configuration")]
#[case::tool_authority("require:shell")]
#[tokio::test]
async fn withdrawn_authority_narrows_to_cancellation_instead_of_driving_effects(
	repository: Repository,
	#[case] boundary: &str,
) {
	repository.0.lock().unwrap().failure = Some((boundary.into(), true));
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.calls.last().unwrap(), "withdraw");
	assert!(trace.saved.is_empty());
	assert!(!trace.commit);
}
#[rstest]
#[case::snapshot("snapshot")]
#[case::begin("begin")]
#[case::current_run("load")]
#[case::runner("health:false:false")]
#[case::persist("persist")]
#[case::finish("finish")]
#[tokio::test]
async fn external_failures_keep_their_identity_without_impersonating_withdrawal(
	repository: Repository,
	#[case] boundary: &str,
) {
	repository.0.lock().unwrap().failure = Some((boundary.into(), false));
	assert!(
		matches!(run(&repository).await,Err(Error::External(message)) if message==format!("fault:{boundary}"))
	);
	let trace = repository.0.lock().unwrap();
	assert!(!trace.calls.contains(&"withdraw".into()));
	assert!(!trace.commit);
}
#[rstest]
#[tokio::test]
async fn mismatched_writer_fences_stop_before_health_or_dispatch(repository: Repository) {
	repository.0.lock().unwrap().area.epoch += 1;
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert!(!trace.calls.iter().any(|call| call.starts_with("health")));
	assert_eq!(trace.calls.last().unwrap(), "withdraw");
}
#[rstest]
#[tokio::test]
async fn changed_runner_identity_records_uncertainty_without_resubmitting(repository: Repository) {
	repository.0.lock().unwrap().health = json!({"instance":"replacement"});
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.saved[0].state, "uncertain");
	assert_eq!(
		trace.saved[0].result,
		json!({"error":"runner journal identity changed"})
	);
	assert!(trace.calls.contains(&"area:uncertain".into()));
	assert!(trace.events.is_empty());
	assert!(!trace.calls.iter().any(|call| call.starts_with("request:")));
}
#[rstest]
#[case::accepted("accepted", "submitted")]
#[case::starting("starting", "submitted")]
#[case::running("running", "running")]
#[case::finishing("finishing", "running")]
#[tokio::test]
async fn active_runner_states_keep_the_original_progress_mapping(
	repository: Repository,
	#[case] observed: &str,
	#[case] expected: &str,
) {
	repository.0.lock().unwrap().observed = json!({"status":observed,"stdout":STANDARD.encode("こんにちは"),"stdout_offset":10,"truncated":true});
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.saved[0].state, expected);
	assert_eq!(
		trace.saved[0].result,
		json!({"preview":"ちは","preview_bytes":STANDARD.encode("ちは"),"preview_offset":19,"truncated":true})
	);
	assert_eq!(trace.events.len(), 1);
	assert!(trace.commit);
}
/// The command's deadline counts from these, and only the runner knows them.
#[rstest]
#[case::shell(json!({"status":"running","stdout":"","started_at":1000.5,"running_at":1002.0}), Some(1000.5))]
#[case::python(json!({"status":"running","stdout":"","running_at":1002.0}), Some(1002.0))]
#[case::not_started(json!({"status":"accepted","stdout":""}), None)]
#[tokio::test]
async fn an_active_operation_keeps_when_its_command_started(
	repository: Repository,
	#[case] observed: Value,
	#[case] started_at: Option<f64>,
) {
	repository.0.lock().unwrap().observed = observed;
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.saved[0].result["started_at"].as_f64(), started_at);
}
#[rstest]
#[tokio::test]
async fn running_preview_preserves_the_runner_bytes_and_absolute_offset(repository: Repository) {
	repository.0.lock().unwrap().observed = json!({"status":"running", "stdout":STANDARD.encode([0xff, b'x']), "stdout_offset":4, "truncated":false});
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(
		trace.saved[0].result,
		json!({"preview":"�x", "preview_bytes":STANDARD.encode([0xff, b'x']), "preview_offset":4, "truncated":false})
	);
}

#[rstest]
#[tokio::test]
async fn running_preview_keeps_a_standalone_invalid_initial_byte(repository: Repository) {
	repository.0.lock().unwrap().observed = json!({"status":"running", "stdout":STANDARD.encode([0x80, b'x']), "stdout_offset":0, "truncated":false});
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(
		trace.saved[0].result,
		json!({"preview":"�x", "preview_bytes":STANDARD.encode([0x80, b'x']), "preview_offset":0, "truncated":false})
	);
}

#[rstest]
#[tokio::test]
async fn a_pending_dispatch_uploads_the_existing_inputs_before_start_and_records_the_observed_result(
	repository: Repository,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.result = json!({"dispatch_pending":true});
		trace.observed = json!({"status":"awaiting_files"});
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	let dispatch = trace
		.calls
		.iter()
		.position(|call| call == "request:POST:/v1/operations")
		.unwrap();
	let upload = trace
		.calls
		.iter()
		.position(|call| call == "upload_files")
		.unwrap();
	let start = trace
		.calls
		.iter()
		.position(|call| call.ends_with("/start"))
		.unwrap();
	assert!(dispatch < upload && upload < start);
	assert_eq!(trace.saved[0].state, "submitted");
	assert_eq!(trace.saved[0].result, json!({}));
}
#[rstest]
#[tokio::test]
async fn absent_runner_state_becomes_uncertain_instead_of_replaying_execution(
	repository: Repository,
) {
	repository.0.lock().unwrap().observed = json!({"status":"absent"});
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.saved[0].state, "uncertain");
	assert_eq!(
		trace.saved[0].result,
		json!({"error":"runner lost a previously accepted operation"})
	);
	assert!(trace.calls.contains(&"area:uncertain".into()));
}
#[rstest]
#[tokio::test]
async fn completed_output_publication_advances_revision_before_persisting_and_emitting_the_change(
	repository: Repository,
) {
	repository.0.lock().unwrap().observed = json!({"status":"completed","termination_confirmed":true,"executed":true,"files":[],"stdout":STANDARD.encode("stdout"),"displays":[],"exit_code":0});
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.saved[0].state, "completed");
	assert_eq!(trace.saved[0].revision, 8);
	assert_eq!(trace.saved[0].result["output_file"]["size"], json!(6));
	assert_eq!(trace.saved[0].result["runner_acknowledged"], json!(false));
	let publish = trace
		.calls
		.iter()
		.position(|call| call == "publish")
		.unwrap();
	let persist = trace
		.calls
		.iter()
		.position(|call| call == "persist")
		.unwrap();
	let event = trace.calls.iter().position(|call| call == "event").unwrap();
	assert!(publish < persist && persist < event);
	assert!(trace.commit);
}
#[rstest]
#[tokio::test]
async fn publication_reuses_an_area_blob_without_downloading_it(repository: Repository) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.reusable = Some(Uuid::from_u128(55));
		trace.area.manifest = json!([{"file_id":Uuid::from_u128(30),"path":"old/path","digest":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad","size":3,"media_type":"application/octet-stream","scope":"working","provenance":{}}]);
		trace.observed = json!({"status":"completed","termination_confirmed":true,"executed":true,
            "files":[{"path":"new/path", "object_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "digest":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", "size":3}],
            "stdout":STANDARD.encode("stdout"), "displays":[], "exit_code":0});
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.published[0].file_id, Uuid::from_u128(55));
	assert_eq!(trace.published[0].path, "new/path");
	assert!(
		!trace
			.calls
			.iter()
			.any(
				|call| (call.starts_with("request:GET:") && call.contains("/files/"))
					|| call == "begin_output"
			)
	);
}

#[rstest]
#[tokio::test]
async fn runner_cannot_reuse_an_unmounted_tenant_blob(repository: Repository) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.reusable = Some(Uuid::from_u128(55));
		trace.observed = json!({"status":"completed","termination_confirmed":true,"executed":true,
			"files":[{"path":"stolen", "object_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "digest":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", "size":3}],
			"stdout":STANDARD.encode("stdout"), "displays":[], "exit_code":0});
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert!(!trace.calls.iter().any(|call| call == "reuse_output"));
	assert!(
		trace
			.calls
			.iter()
			.any(|call| call.starts_with("request:GET:") && call.contains("/files/"))
	);
	assert!(trace.calls.iter().any(|call| call == "finish_output"));
	assert_ne!(trace.published[0].file_id, Uuid::from_u128(55));
}

#[rstest]
#[tokio::test]
async fn mounted_operation_input_can_be_reused_without_download(repository: Repository) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.reusable = Some(Uuid::from_u128(55));
		trace.files = serde_json::from_value(json!([{"file_id":Uuid::from_u128(30),"path":"mounted","digest":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad","size":3,"media_type":"application/octet-stream","scope":"references","provenance":{}}])).unwrap();
		trace.observed = json!({"status":"completed","termination_confirmed":true,"executed":true,
			"files":[{"path":"copy", "object_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "digest":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", "size":3}],
			"stdout":STANDARD.encode("stdout"), "displays":[], "exit_code":0});
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.published[0].file_id, Uuid::from_u128(55));
	assert!(
		!trace
			.calls
			.iter()
			.any(|call| call.starts_with("request:GET:") && call.contains("/files/"))
	);
}

#[rstest]
#[case::completed("completed")]
#[case::cancelled("cancelled")]
#[case::failed("failed")]
#[tokio::test]
async fn confirmed_unexecuted_operations_need_no_output_publication(
	repository: Repository,
	#[case] status: &str,
) {
	repository.0.lock().unwrap().observed =
		json!({"status":status,"termination_confirmed":true,"executed":false});
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.saved[0].state, status);
	assert_eq!(
		trace.saved[0].result,
		json!({"termination_confirmed":true,"effects_may_have_occurred":false,"runner_acknowledged":false})
	);
	assert!(trace.published.is_empty());
	assert!(trace.events.is_empty());
}

#[rstest]
#[tokio::test]
async fn unexecuted_python_failure_releases_its_session(repository: Repository) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.kind = "code_interpreter".into();
		trace.observed = json!({"status":"failed","termination_confirmed":true,"executed":false,
			"error":{"code":"STARTUP_TIMEOUT","message":"Startup timed out"}});
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert!(trace.calls.iter().any(|call| call == "python:false"));
	assert_eq!(
		trace.saved[0].result["error"]["message"],
		"Startup timed out"
	);
	assert!(trace.published.is_empty());
}
#[rstest]
#[tokio::test]
async fn a_shell_writer_is_not_confirmed_stopped_by_a_python_freeze_claim(repository: Repository) {
	repository.0.lock().unwrap().observed =
		json!({"status":"completed","writer_frozen":true,"termination_confirmed":false});
	assert!(
		matches!(run(&repository).await,Err(Error::External(message)) if message=="invalid runner operation state")
	);
	let trace = repository.0.lock().unwrap();
	assert!(trace.saved.is_empty());
	assert!(!trace.commit);
}
#[rstest]
#[tokio::test]
async fn cancelling_a_running_operation_uses_the_same_runner_identity_and_cancel_endpoint(
	repository: Repository,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.state = "cancelling".into();
		trace.observed = json!({"status":"running","stdout":""});
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert!(trace.calls.contains(&"health:true:false".into()));
	assert!(
		trace
			.calls
			.iter()
			.any(|call| call.starts_with("request:POST:") && call.ends_with("/cancel"))
	);
	assert_eq!(trace.saved[0].state, "cancelling");
	assert!(!trace.calls.contains(&"configuration".into()));
}
/// However the stop came about, an operation that may not have reached the
/// Runner is never sent. The Runner refuses a later send and answers that the
/// command did not run, so nothing changed and the Run goes on.
#[rstest]
#[case::agent_cancel("cancelling", RunControl::Active, RunPhase::Ready, true)]
#[case::person_cancelled_the_run("submitted", RunControl::Cancelled, RunPhase::Ready, true)]
#[case::run_ended("submitted", RunControl::Active, RunPhase::Completed, true)]
#[case::admission_off("submitted", RunControl::Active, RunPhase::Ready, false)]
#[tokio::test]
async fn a_stopped_pending_operation_is_refused_and_ends_without_effects(
	repository: Repository,
	#[case] state: &str,
	#[case] control: RunControl,
	#[case] phase: RunPhase,
	#[case] admission: bool,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.state = state.into();
		trace.operation.result = json!({"dispatch_pending":true});
		trace.run.control = control;
		trace.run.phase = phase;
		trace.limits.admission = admission;
		trace.observed =
			json!({"status":"cancelled","termination_confirmed":true,"executed":false});
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(
		trace.bodies,
		vec![json!({"digest":"digest","instance":"runner"})]
	);
	assert!(
		!trace
			.calls
			.contains(&"request:POST:/v1/operations".to_owned())
	);
	assert_eq!(trace.saved[0].state, "cancelled");
	assert_eq!(
		trace.saved[0].result,
		json!({"termination_confirmed":true,"effects_may_have_occurred":false,"runner_acknowledged":false})
	);
	assert!(trace.calls.contains(&"area:active".to_owned()));
	assert!(trace.published.is_empty());
	assert!(trace.commit);
}
/// A refusal the server never committed (its reply was lost, or the commit
/// failed) leaves the operation unsent in the journal. The next pass sends it
/// again, and the Runner answers with the refusal it recorded. That settles the
/// operation: it neither runs nor stays pending.
#[rstest]
#[tokio::test]
async fn a_resent_operation_the_runner_already_refused_ends_without_effects(
	repository: Repository,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.result = json!({"dispatch_pending":true});
		trace.observed = json!({"operation_id":Uuid::from_u128(1),"digest":"digest","status":"cancelled",
			"termination_confirmed":true,"executed":false});
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert!(
		trace
			.calls
			.contains(&"request:POST:/v1/operations".to_owned())
	);
	assert!(!trace.calls.iter().any(|call| call.ends_with("/start")));
	assert!(!trace.calls.contains(&"upload_files".to_owned()));
	assert_eq!(trace.saved[0].state, "cancelled");
	assert_eq!(
		trace.saved[0].result,
		json!({"termination_confirmed":true,"effects_may_have_occurred":false,"runner_acknowledged":false})
	);
	assert!(trace.calls.contains(&"area:active".to_owned()));
	assert!(trace.published.is_empty());
	assert!(trace.commit);
}
#[rstest]
#[case::shell("shell", None)]
#[case::check(VERIFICATION, None)]
#[case::package_install("python_install", None)]
#[case::python_cell("code_interpreter", Some("python:false"))]
#[tokio::test]
async fn every_kind_of_pending_operation_ends_the_same_way(
	repository: Repository,
	#[case] kind: &str,
	#[case] python: Option<&str>,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.kind = kind.into();
		trace.operation.state = "cancelling".into();
		trace.operation.result = json!({"dispatch_pending":true});
		trace.observed =
			json!({"status":"cancelled","termination_confirmed":true,"executed":false});
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.saved[0].state, "cancelled");
	assert!(trace.calls.contains(&"area:active".to_owned()));
	assert_eq!(
		trace
			.calls
			.iter()
			.find(|call| call.starts_with("python:"))
			.map(String::as_str),
		python
	);
}
/// A send that did arrive is an ordinary cancel, and the reply clears the intent.
#[rstest]
#[tokio::test]
async fn a_pending_operation_the_runner_holds_is_cancelled_as_usual(repository: Repository) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.state = "cancelling".into();
		trace.operation.result = json!({"dispatch_pending":true});
		trace.observed = json!({"status":"accepted","stdout":""});
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.saved[0].state, "cancelling");
	assert_eq!(trace.saved[0].result["dispatch_pending"], Value::Null);
}
#[rstest]
#[tokio::test]
async fn a_cancel_after_the_runner_answered_asks_for_no_refusal(repository: Repository) {
	repository.0.lock().unwrap().operation.state = "cancelling".into();
	run(&repository).await.unwrap();
	assert!(repository.0.lock().unwrap().bodies.is_empty());
}
/// A replaced journal proves nothing about what the old Runner did, so neither
/// its absence nor a new Runner's word can say that nothing ran.
#[rstest]
#[case::runner_replaced(json!({"instance":"replacement"}), json!({"status":"cancelled","termination_confirmed":true,"executed":false}))]
#[case::runner_says_absent(json!({"instance":"runner"}), json!({"status":"absent"}))]
#[tokio::test]
async fn a_pending_cancel_the_runner_cannot_prove_stays_uncertain(
	repository: Repository,
	#[case] health: Value,
	#[case] observed: Value,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.state = "cancelling".into();
		trace.operation.result = json!({"dispatch_pending":true});
		trace.health = health;
		trace.observed = observed;
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.saved[0].state, "uncertain");
	assert!(trace.calls.contains(&"area:uncertain".to_owned()));
}
#[rstest]
#[tokio::test]
async fn short_input_reads_stop_before_starting_the_runner(repository: Repository) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.observed = json!({"status":"awaiting_files"});
		trace.files = vec![MountedFile {
			file_id: Uuid::from_u128(11),
			path: "input".into(),
			digest: "digest".into(),
			size: 1,
			media_type: "text/plain".into(),
			scope: FileScope::Working,
			provenance: json!({}),
		}];
	}
	assert!(
		matches!(run(&repository).await,Err(Error::Conflict(message)) if message=="OBJECT_INTEGRITY")
	);
	let trace = repository.0.lock().unwrap();
	assert!(!trace.calls.iter().any(|call| call.ends_with("/start")));
	assert!(!trace.commit);
}
#[rstest]
#[tokio::test]
async fn oversized_previews_fail_without_publishing_or_journaling_a_result(repository: Repository) {
	repository.0.lock().unwrap().observed =
		json!({"status":"running","stdout":STANDARD.encode(vec![b'x';16385])});
	assert!(
		matches!(run(&repository).await,Err(Error::Invalid(message)) if message=="runner preview limit")
	);
	let trace = repository.0.lock().unwrap();
	assert!(trace.saved.is_empty());
	assert!(!trace.commit);
}
#[rstest]
#[tokio::test]
async fn a_completion_check_dispatches_the_verify_kind_under_shell_authority(
	repository: Repository,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.kind = VERIFICATION.into();
		trace.operation.result = json!({"dispatch_pending":true});
		trace.observed = json!({"status":"accepted","stdout":""});
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert!(trace.calls.contains(&"require:shell".into()));
	assert_eq!(trace.bodies[0]["kind"], "verify");
	assert_eq!(trace.bodies[0]["code"], "echo fixture");
	assert!(trace.commit);
}
/// The runner's runtime is kept in milliseconds, so a Check that fails in
/// under a second still spends verification time. It ends when the runner saw
/// the command stop; harvesting its output is not charged.
#[rstest]
#[case::fractional(1000.25, None, 1004.75, 4500, None)]
#[case::sub_second(1000.0, None, 1000.4, 400, None)]
#[case::instant(1000.0, None, 1000.0, 0, None)]
#[case::slow_harvest(1000.0, Some(1000.4), 1045.0, 400, Some(false))]
#[case::deadline(1000.0, Some(1003.0), 1003.2, 3000, Some(true))]
#[tokio::test]
async fn a_finished_completion_check_keeps_its_output_tail_and_never_publishes(
	repository: Repository,
	#[case] started_at: f64,
	#[case] terminated_at: Option<f64>,
	#[case] finished_at: f64,
	#[case] runtime_millis: u64,
	#[case] deadline_expired: Option<bool>,
) {
	let output = format!("{}test result: FAILED", "x".repeat(MAX_DIAGNOSTIC_BYTES));
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.kind = VERIFICATION.into();
		trace.operation.state = "running".into();
		trace.observed = json!({"status":"failed","termination_confirmed":true,"executed":true,
			"exit_code":101,"truncated":false,"error":null,"stdout":STANDARD.encode(&output),
			"started_at":started_at,"finished_at":finished_at,
			"files":[{"path":"target/debug/app","object_id":"build","digest":"d","size":1}]});
		if let Some(terminated_at) = terminated_at {
			trace.observed["terminated_at"] = json!(terminated_at);
		}
		if let Some(expired) = deadline_expired {
			trace.observed["deadline_expired"] = json!(expired);
		}
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert!(
		!trace
			.calls
			.iter()
			.any(|call| call == "publish" || call.starts_with("put:"))
	);
	assert!(trace.calls.contains(&"area:active".into()));
	let saved = &trace.saved[0];
	assert_eq!(saved.state, "failed");
	assert_eq!(saved.revision, 7, "a Check never changes the Area revision");
	assert_eq!(saved.result["exit_code"], 101);
	assert_eq!(
		saved.result["runtime_millis"], runtime_millis,
		"the runner's own runtime"
	);
	assert_eq!(
		saved.result["deadline_expired"].as_bool(),
		deadline_expired,
		"the runner's word on the deadline"
	);
	let preview = saved.result["preview"].as_str().unwrap();
	assert_eq!(preview.len(), MAX_DIAGNOSTIC_BYTES);
	assert!(preview.ends_with("test result: FAILED"));
	assert!(trace.published.is_empty());
	assert_eq!(trace.events.len(), 1);
	assert!(trace.commit);
}
/// What the runner reports when a command's Time Limit passed while it ran.
fn past_its_limit(status: &str, exit_code: i64) -> Value {
	json!({"status":status,"termination_confirmed":true,"executed":true,"files":[],
		"stdout":STANDARD.encode("stdout"),"displays":[],"exit_code":exit_code,"error":null,
		"deadline_expired":true})
}
/// The limit decides, whatever exit code the command then gave and whoever
/// stopped it. Its files are published, so the error can say they were saved.
#[rstest]
#[case::killed("failed", 143)]
#[case::exited_zero_after_the_limit("completed", 0)]
#[case::stopped_by_the_runner("cancelled", 137)]
#[tokio::test]
async fn a_command_past_its_limit_saves_its_files_and_reports_the_time_out(
	repository: Repository,
	#[case] status: &str,
	#[case] exit_code: i64,
) {
	repository.0.lock().unwrap().observed = past_its_limit(status, exit_code);
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	let saved = &trace.saved[0];
	assert_eq!(saved.state, "failed");
	assert_eq!(saved.revision, 8);
	assert_eq!(saved.result["exit_code"], exit_code);
	assert_eq!(saved.result["deadline_expired"], true);
	assert_eq!(saved.result["error"]["code"], COMMAND_TIMED_OUT);
	assert_eq!(saved.result["error"]["retryable"], false);
	assert_eq!(
		saved.result["error"]["details"],
		json!({"limit_seconds":1,"revision":8})
	);
	let message = saved.result["error"]["message"].as_str().unwrap();
	assert!(message.contains("revision 8"), "{message}");
	assert!(message.contains("at most 600"), "{message}");
	assert!(trace.calls.contains(&"publish".into()));
	assert!(trace.commit);
}
#[rstest]
#[tokio::test]
async fn a_python_cell_past_its_limit_reports_the_time_out_and_its_reset_session(
	repository: Repository,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.kind = "code_interpreter".into();
		trace.operation.input["session_id"] = json!(Uuid::from_u128(20));
		trace.observed = past_its_limit("failed", 137);
		trace.observed["error"] = json!({"code":"SESSION_LOST","message":"Interpreter stopped"});
		trace.observed["writer_frozen"] = json!(false);
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	let saved = &trace.saved[0];
	assert_eq!(saved.state, "failed");
	assert_eq!(saved.result["error"]["code"], COMMAND_TIMED_OUT);
	let message = saved.result["error"]["message"].as_str().unwrap();
	assert!(message.contains("session was reset"), "{message}");
	assert!(trace.calls.contains(&"python:false".into()));
}
/// A command that ended before its limit keeps the runner's own word, and its
/// result does not grow a field that says nothing happened.
#[rstest]
#[case::cancelled("cancelled", 137, json!({"code":"CANCELLED","message":"Stopped"}), Some(false))]
#[case::failed("failed", 1, Value::Null, Some(false))]
#[case::the_runner_could_not_tell("failed", 1, Value::Null, None)]
#[tokio::test]
async fn a_command_that_ended_before_its_limit_keeps_its_outcome(
	repository: Repository,
	#[case] status: &str,
	#[case] exit_code: i64,
	#[case] error: Value,
	#[case] deadline_expired: Option<bool>,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.observed = past_its_limit(status, exit_code);
		trace.observed["error"] = error.clone();
		match deadline_expired {
			Some(expired) => trace.observed["deadline_expired"] = json!(expired),
			None => {
				trace
					.observed
					.as_object_mut()
					.unwrap()
					.remove("deadline_expired");
			}
		}
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	let saved = &trace.saved[0];
	assert_eq!(saved.state, status);
	assert_eq!(saved.result["error"], error);
	assert!(saved.result.get("deadline_expired").is_none());
}
/// Nothing was saved, so the refusal stays the error and the limit is added to it.
#[rstest]
#[tokio::test]
async fn a_command_past_its_limit_whose_save_the_runner_refused_says_both(repository: Repository) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.observed = refused_export("symlink", "a", Value::Null);
		trace.observed["deadline_expired"] = json!(true);
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	let saved = &trace.saved[0];
	assert_eq!(saved.state, "failed");
	assert_eq!(saved.revision, 7);
	assert_eq!(saved.result["deadline_expired"], true);
	assert_eq!(saved.result["error"]["code"], "WORKING_EXPORT_REFUSED");
	let message = saved.result["error"]["message"].as_str().unwrap();
	assert!(
		message.starts_with("symlinks are not supported"),
		"{message}"
	);
	assert!(
		message.ends_with("under /tmp. The command had also reached its 1 s time limit."),
		"{message}"
	);
	assert!(!trace.calls.contains(&"publish".into()));
}
#[rstest]
#[tokio::test]
async fn a_command_past_its_limit_over_the_quota_says_both(repository: Repository) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.observed = past_its_limit("failed", 143);
		trace.observed["files"] = json!([working_file("big", 2048)]);
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	let saved = &trace.saved[0];
	assert_eq!(saved.state, "failed");
	assert_eq!(saved.revision, 7);
	assert_eq!(saved.result["deadline_expired"], true);
	assert_eq!(saved.result["error"]["code"], "WORKING_EXPORT_REFUSED");
	let message = saved.result["error"]["message"].as_str().unwrap();
	assert!(message.contains("1 s time limit"), "{message}");
}
/// A Check saves nothing and has its own timed-out outcome, so it never gets
/// the time-out error, which would say that files were saved.
#[rstest]
#[tokio::test]
async fn a_completion_check_past_its_limit_gets_no_time_out_error(repository: Repository) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.kind = VERIFICATION.into();
		trace.observed = past_its_limit("failed", 143);
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	let saved = &trace.saved[0];
	assert_eq!(saved.result["deadline_expired"], true);
	assert!(saved.result["error"].is_null());
}
/// A Check never exports `/work`, so losing its journal leaves the Area's files
/// as they were: the Check's result is unknown, but the Area stays usable for
/// the next Check or tool call.
#[rstest]
#[case::identity_changed(json!({"instance":"replacement"}), json!({"status":"running","stdout":""}))]
#[case::absent(json!({"instance":"runner"}), json!({"status":"absent"}))]
#[case::runner_uncertain(json!({"instance":"runner"}), json!({"status":"uncertain","error":"sandbox lost","termination_confirmed":true}))]
#[tokio::test]
async fn a_lost_completion_check_leaves_the_area_usable(
	repository: Repository,
	#[case] health: Value,
	#[case] observed: Value,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.kind = VERIFICATION.into();
		trace.health = health;
		trace.observed = observed;
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.saved[0].state, "uncertain");
	assert!(trace.calls.contains(&"area:active".into()));
	assert!(!trace.calls.contains(&"area:uncertain".into()));
	assert!(trace.published.is_empty());
}
/// A lost Check whose writer the runner has not confirmed stopped keeps the
/// Area fenced: the runner would refuse the next Check while it holds that
/// writer, so a retry must not be offered an Area it can never run in. The
/// Check stays cancelling, so reconciliation keeps asking the runner to prove
/// the writer stopped instead of leaving the Area fenced for good.
#[rstest]
#[case::unconfirmed(json!({"status":"uncertain","error":"sandbox lost","termination_confirmed":false}))]
#[case::unreported(json!({"status":"uncertain","error":"sandbox lost"}))]
#[tokio::test]
async fn a_lost_completion_check_with_a_live_writer_keeps_the_area_fenced(
	repository: Repository,
	#[case] observed: Value,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.kind = VERIFICATION.into();
		trace.health = json!({"instance":"runner"});
		trace.observed = observed;
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.saved[0].state, "cancelling");
	assert!(trace.saved[0].active(), "the held Check must stay polled");
	assert!(trace.calls.contains(&"area:uncertain".into()));
	assert!(!trace.calls.contains(&"area:active".into()));
	assert!(trace.published.is_empty());
}
/// The passes after a held Check: the runner retries the stop proof while it
/// reports `cancelling`, and once it proves the writer stopped the Area opens
/// again without publishing anything.
#[rstest]
#[tokio::test]
async fn a_held_completion_check_reopens_the_area_once_its_writer_is_proven_stopped(
	repository: Repository,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.kind = VERIFICATION.into();
		trace.operation.state = "cancelling".into();
		trace.health = json!({"instance":"runner"});
		trace.observed = json!({"status":"cancelling","stdout":"","termination_confirmed":false});
	}
	run(&repository).await.unwrap();
	{
		let mut trace = repository.0.lock().unwrap();
		assert!(
			trace
				.calls
				.iter()
				.any(|call| call.starts_with("request:POST:") && call.ends_with("/cancel"))
		);
		assert_eq!(trace.saved[0].state, "cancelling");
		assert!(!trace.calls.contains(&"area:active".into()));
		trace.operation = trace.saved[0].clone();
		trace.saved.clear();
		trace.calls.clear();
		trace.observed = json!({"status":"failed","termination_confirmed":true,"executed":true,
			"exit_code":137,"truncated":true,"stdout":"",
			"error":{"code":"SANDBOX_LOST","message":"Sandbox stopped"}});
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.saved[0].state, "failed");
	assert_eq!(trace.saved[0].result["error"]["code"], "SANDBOX_LOST");
	assert!(trace.calls.contains(&"area:active".into()));
	assert!(trace.published.is_empty());
}
/// Only a Check is kept polling: a shell writer may have changed `/work`, so
/// its lost outcome still fences the Area until a person resolves it.
#[rstest]
#[tokio::test]
async fn a_lost_shell_operation_with_a_live_writer_stays_uncertain(repository: Repository) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.health = json!({"instance":"runner"});
		trace.observed =
			json!({"status":"uncertain","error":"sandbox lost","termination_confirmed":false});
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.saved[0].state, "uncertain");
	assert!(trace.calls.contains(&"area:uncertain".into()));
}

#[rstest]
#[tokio::test]
async fn unconfirmed_stop_stays_active_and_keeps_the_area_busy(repository: Repository) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.state = "running".into();
		trace.observed = json!({"status":"cancelling","stdout":"","termination_confirmed":false,"output_fetched_at":1234});
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.saved[0].state, "cancelling");
	assert!(trace.saved[0].active());
	assert_eq!(trace.saved[0].result["output_fetched_at"], 1234);
	assert_eq!(trace.saved[0].result["error"]["code"], "STOP_UNCONFIRMED");
	assert!(!trace.calls.iter().any(|call| call.starts_with("area:")));
	assert!(trace.published.is_empty());
}

#[rstest]
#[tokio::test]
async fn stopped_uncertain_shell_reopens_without_publishing_and_preserves_error(
	repository: Repository,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.state = "running".into();
		trace.observed = json!({"status":"uncertain","termination_confirmed":true,
			"error":{"code":"OUTPUT_LOST","message":"File changes were not saved."}});
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.saved[0].state, "uncertain");
	assert_eq!(
		trace.saved[0].result["error"]["message"],
		"File changes were not saved."
	);
	assert!(trace.calls.contains(&"area:active".into()));
	assert!(trace.published.is_empty());
}

#[rstest]
#[tokio::test]
async fn stopped_uncertain_python_resets_its_session_without_publishing(repository: Repository) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.kind = "code_interpreter".into();
		trace.operation.state = "running".into();
		trace.observed = json!({"status":"uncertain","termination_confirmed":true,
			"error":{"code":"OUTPUT_LOST","message":"File changes were not saved."}});
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.saved[0].state, "uncertain");
	assert!(trace.calls.contains(&"area:active".into()));
	assert!(trace.calls.contains(&"python:false".into()));
	assert!(trace.published.is_empty());
}
/// What the runner reports when its collector refused to export `/work` (#682).
fn refused_export(reason: &str, path: &str, limit: Value) -> Value {
	json!({"status":"failed","termination_confirmed":true,"executed":true,"exit_code":0,
		"stdout":STANDARD.encode("stdout"),"truncated":false,
		"error":{"code":"WORKING_EXPORT_REFUSED","retryable":false,
			"message":format!("symlinks are not supported in /work: {path}; use npm install --no-bin-links, or keep dependency trees under /tmp"),
			"details":{"reason":reason,"path":path,"limit":limit}}})
}
#[rstest]
#[tokio::test]
async fn a_refused_export_fails_only_the_operation_and_keeps_the_published_revision(
	repository: Repository,
) {
	repository.0.lock().unwrap().observed =
		refused_export("symlink", "tools/node_modules/.bin/sha.js", Value::Null);
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	let saved = &trace.saved[0];
	assert_eq!(saved.state, "failed");
	assert_eq!(saved.revision, 7);
	assert_eq!(saved.result["error"]["code"], "WORKING_EXPORT_REFUSED");
	assert_eq!(
		saved.result["error"]["details"],
		json!({"reason":"symlink","path":"tools/node_modules/.bin/sha.js","limit":null})
	);
	assert_eq!(saved.result["termination_confirmed"], true);
	// The command's own output stays readable beside the refusal.
	assert_eq!(saved.result["output_file"]["size"], json!(6));
	assert!(trace.calls.contains(&"area:active".into()));
	assert!(!trace.calls.contains(&"publish".into()));
	assert!(
		!trace
			.calls
			.iter()
			.any(|call| call.starts_with("area:uncertain"))
	);
	assert_eq!(trace.events.len(), 1);
	assert!(trace.commit);
}
#[rstest]
#[tokio::test]
async fn a_refused_python_export_resets_the_session_without_publishing(repository: Repository) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.kind = "code_interpreter".into();
		trace.operation.input =
			json!({"command":"print(1)","seconds":1,"session_id":Uuid::from_u128(20)});
		trace.observed = refused_export("hard_link", "a", Value::Null);
		trace.observed["writer_frozen"] = json!(false);
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.saved[0].state, "failed");
	assert_eq!(trace.saved[0].revision, 7);
	assert_eq!(trace.saved[0].result["writer_frozen"], false);
	assert!(trace.calls.contains(&"python:false".into()));
	assert!(trace.calls.contains(&"area:active".into()));
	assert!(!trace.calls.contains(&"publish".into()));
}
fn working_file(path: &str, size: u64) -> Value {
	json!({"path":path,"size":size,"object_id":"0".repeat(64),"digest":"digest"})
}
#[rstest]
#[tokio::test]
async fn an_export_over_the_file_ceiling_is_refused_before_any_download(repository: Repository) {
	{
		let mut trace = repository.0.lock().unwrap();
		// A mounted reference counts toward the manifest's 4096 entries.
		trace.area.manifest = json!([{"file_id":Uuid::from_u128(30),"path":"guide.md","digest":"d",
			"size":1,"media_type":"text/plain","scope":"references","provenance":{}}]);
		trace.limits.working_bytes = 1 << 20;
		let files = (0..4096)
			.map(|index| working_file(&format!("f{index:04}"), 0))
			.collect::<Vec<_>>();
		trace.observed = json!({"status":"completed","termination_confirmed":true,"executed":true,
			"files":files,"stdout":STANDARD.encode("stdout"),"displays":[],"exit_code":0});
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	let saved = &trace.saved[0];
	assert_eq!(saved.state, "failed");
	assert_eq!(saved.revision, 7);
	assert_eq!(saved.result["error"]["code"], "WORKING_EXPORT_REFUSED");
	assert_eq!(
		saved.result["error"]["details"],
		json!({"reason":"file_count","path":"f4095","limit":4096})
	);
	assert!(
		saved.result["error"]["message"]
			.as_str()
			.unwrap()
			.contains("at most 4096 files")
	);
	assert!(!trace.calls.contains(&"begin_output".into()));
	assert!(!trace.calls.contains(&"publish".into()));
	assert!(trace.calls.contains(&"area:active".into()));
}
#[test]
fn reference_files_leave_the_whole_quota_to_an_export() {
	let mounted = |scope, kind: &str| MountedFile {
		file_id: Uuid::from_u128(77),
		path: "references/manual.pdf".into(),
		digest: "a".repeat(64),
		size: 5000,
		media_type: "application/pdf".into(),
		scope,
		provenance: json!({"kind":kind}),
	};
	let exported = [working_file("small", 24)];
	assert!(
		quota_refusal(
			&[mounted(FileScope::References, "reference")],
			&exported,
			1024
		)
		.unwrap()
		.is_none()
	);
	assert!(
		quota_refusal(
			&[mounted(FileScope::Received, "transfer_in")],
			&exported,
			1024
		)
		.unwrap()
		.is_some()
	);
}
#[rstest]
#[tokio::test]
async fn an_export_over_the_byte_quota_is_refused_before_any_download(repository: Repository) {
	repository.0.lock().unwrap().observed = json!({"status":"completed","termination_confirmed":true,
		"executed":true,"files":[working_file("small",24),working_file("large",1001)],
		"stdout":STANDARD.encode("stdout"),"displays":[],"exit_code":0});
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	let saved = &trace.saved[0];
	assert_eq!(saved.state, "failed");
	assert_eq!(
		saved.result["error"]["details"],
		json!({"reason":"bytes","path":"large","limit":1024})
	);
	assert!(!trace.calls.contains(&"begin_output".into()));
	assert!(!trace.calls.contains(&"publish".into()));
}

#[rstest]
#[tokio::test]
async fn a_timed_out_install_gets_install_advice(repository: Repository) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.kind = "python_install".into();
		trace.observed = past_its_limit("failed", 143);
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	let message = trace.saved[0].result["error"]["message"].as_str().unwrap();
	assert!(message.contains("wheels"), "{message}");
	assert!(message.contains("complete"), "{message}");
	assert!(message.contains("dependencies"), "{message}");
	assert!(!message.contains("at a time"), "{message}");
	assert!(!message.contains("shorter commands"), "{message}");
	assert!(trace.calls.contains(&"require:code_interpreter".into()));
	assert!(trace.calls.contains(&"packages".into()));
}

#[rstest]
#[tokio::test]
async fn timeout_advice_respects_the_binding_ceiling(repository: Repository) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.timeout_ceiling = Some(120);
		trace.operation.input["seconds"] = json!(120);
		trace.observed = past_its_limit("failed", 143);
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	let message = trace.saved[0].result["error"]["message"].as_str().unwrap();
	assert!(!message.contains("raise timeout_seconds"), "{message}");
}
