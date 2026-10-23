use super::*;
use aidash_domain::{
	RunControl, RunMetadata, RunPhase,
	capabilities::{
		CoreCapabilities,
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
	capabilities: CoreCapabilities,
	health: Value,
	observed: Value,
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
	pod_times: Vec<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
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
		},
		capabilities: CoreCapabilities {
			shell: true,
			python: true,
			..Default::default()
		},
		health: json!({"instance":"runner"}),
		observed: json!({"status":"running","stdout":""}),
		files: vec![],
		chunks: vec![],
		calls: vec![],
		saved: vec![],
		events: vec![],
		published: vec![],
		commit: false,
		failure: None,
		bodies: vec![],
		pod_times: vec![],
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
		if path == "/v1/operations" {
			assert!(body.as_ref().unwrap()["operation_id"] == json!(Uuid::from_u128(1)));
		}
		let mut trace = self.0.0.lock().unwrap();
		trace.bodies.extend(body);
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
	async fn pod_time(
		&mut self,
		operation: Uuid,
		started_at: chrono::DateTime<chrono::Utc>,
		ended_at: chrono::DateTime<chrono::Utc>,
	) -> Result<()> {
		assert_eq!(operation, Uuid::from_u128(1));
		self.0.call("pod_time")?;
		self.0
			.0
			.lock()
			.unwrap()
			.pod_times
			.push((started_at, ended_at));
		Ok(())
	}
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()> {
		self.0.call("finish")?;
		self.0.0.lock().unwrap().commit = result.is_ok();
		result
	}
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
	repository.0.lock().unwrap().observed =
		json!({"status":observed,"stdout":STANDARD.encode("こんにちは"),"truncated":true});
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.saved[0].state, expected);
	assert_eq!(
		trace.saved[0].result,
		json!({"preview":"こん","truncated":true})
	);
	assert_eq!(trace.events.len(), 1);
	assert!(trace.commit);
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
/// The runner's Pod start and stop, for every kind of operation, are what
/// the Execution Pod's time is metered from.
#[rstest]
#[case::shell("shell")]
#[case::python("code_interpreter")]
#[case::check(VERIFICATION)]
#[tokio::test]
async fn a_finished_operation_reports_the_time_its_pod_ran(
	repository: Repository,
	#[case] kind: &str,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.operation.kind = kind.into();
		trace.operation.state = "running".into();
		trace.observed = json!({"status":"completed","termination_confirmed":true,"executed":true,
			"files":[],"stdout":STANDARD.encode("stdout"),"displays":[],"exit_code":0,
			"started_at":1000.25,"finished_at":1004.75});
	}
	run(&repository).await.unwrap();
	let trace = repository.0.lock().unwrap();
	let [(started, ended)] = trace.pod_times[..] else {
		panic!("one report: {:?}", trace.pod_times);
	};
	assert_eq!(started.timestamp_millis(), 1_000_250);
	assert_eq!(ended.timestamp_millis(), 1_004_750);
}
#[rstest]
#[tokio::test]
async fn an_operation_that_never_ran_or_has_no_runner_times_reports_nothing(
	repository: Repository,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.observed = json!({"status":"cancelled","termination_confirmed":true,"executed":false,
			"started_at":1000.0,"finished_at":1001.0});
	}
	run(&repository).await.unwrap();
	assert!(repository.0.lock().unwrap().pod_times.is_empty());
	let repository = repository.clone();
	{
		let mut trace = repository.0.lock().unwrap();
		trace.observed = json!({"status":"completed","termination_confirmed":true,"executed":true,
			"files":[],"stdout":STANDARD.encode("stdout"),"displays":[],"exit_code":0});
	}
	run(&repository).await.unwrap();
	assert!(repository.0.lock().unwrap().pod_times.is_empty());
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
