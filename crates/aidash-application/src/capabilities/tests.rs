use super::*;
use aidash_domain::{
	RunControl, RunPhase,
	capabilities::operations::{AdmissionLimits, AreaSnapshot, FileScope, MountedFile},
};
use async_trait::async_trait;
use chrono::Utc;
use rstest::{fixture, rstest};
use uuid::Uuid;

struct Control {
	operation: aidash_domain::capabilities::operations::OperationState,
	result: Value,
	calls: Vec<&'static str>,
	failure: Option<&'static str>,
	offset: Option<usize>,
}
#[fixture]
fn control() -> Control {
	Control {
		operation: aidash_domain::capabilities::operations::OperationState {
			area_id: Uuid::from_u128(11),
			run_id: Uuid::from_u128(12),
			principal: "subject".into(),
			kind: "code_interpreter".into(),
			state: "prepared".into(),
		},
		result: json!({"prior":"unchanged"}),
		calls: vec![],
		failure: None,
		offset: None,
	}
}
impl Control {
	fn record(&mut self, stage: &'static str) -> Result<()> {
		self.calls.push(stage);
		if self.failure == Some(stage) {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
}
#[async_trait]
impl crate::ports::capabilities::OperationControlScope for Control {
	fn area_id(&self) -> Uuid {
		Uuid::from_u128(11)
	}
	fn run_id(&self) -> Uuid {
		Uuid::from_u128(12)
	}
	fn principal(&self) -> &str {
		"subject"
	}
	async fn load(
		&mut self,
		id: Uuid,
	) -> Result<aidash_domain::capabilities::operations::OperationState> {
		assert_eq!(id, Uuid::from_u128(13));
		self.record("load")?;
		Ok(self.operation.clone())
	}
	fn apply_cancellation(
		&mut self,
		change: aidash_domain::capabilities::operations::Cancellation,
	) -> Result<()> {
		self.record("change")?;
		self.operation.state = change.state.into();
		if let Some(result) = change.result {
			self.result = result;
		}
		Ok(())
	}
	async fn activate_area(&mut self) -> Result<()> {
		self.record("activate")
	}
	async fn complete_python_without_writer(&mut self) -> Result<()> {
		self.record("python")
	}
	async fn persist(&mut self) -> Result<()> {
		self.record("persist")
	}
	async fn project(&mut self, offset: usize) -> Result<Value> {
		self.record("project")?;
		self.offset = Some(offset);
		Ok(json!({"state":self.operation.state,"result":self.result}))
	}
}
#[rstest]
#[case::python("code_interpreter", true)]
#[case::shell("shell", false)]
#[tokio::test]
async fn cancelling_prepared_operations_releases_the_area_before_persisting_a_safe_result(
	mut control: Control,
	#[case] kind: &str,
	#[case] python: bool,
) {
	control.operation.kind = kind.into();
	let result = poll(&mut control, Uuid::from_u128(13), None, kind, true)
		.await
		.unwrap();
	assert_eq!(
		result,
		json!({"state":"cancelled","result":{"termination_confirmed":true,"effects_may_have_occurred":false}})
	);
	let mut expected = vec!["load", "change", "activate"];
	if python {
		expected.push("python");
	}
	expected.extend(["persist", "project"]);
	assert_eq!(control.calls, expected);
	assert_eq!(control.offset, Some(0));
}
#[rstest]
#[case::submitted("submitted")]
#[case::running("running")]
#[case::unknown("unknown")]
#[tokio::test]
async fn dispatched_cancellation_preserves_saved_results_and_leaves_writer_reconciliation_pending(
	mut control: Control,
	#[case] state: &str,
) {
	control.operation.state = state.into();
	assert_eq!(
		poll(
			&mut control,
			Uuid::from_u128(13),
			Some(17),
			"code_interpreter",
			true
		)
		.await
		.unwrap(),
		json!({"state":"cancelling","result":{"prior":"unchanged"}})
	);
	assert_eq!(control.calls, vec!["load", "change", "persist", "project"]);
	assert_eq!(control.offset, Some(17));
}
#[rstest]
#[case::completed("completed")]
#[case::cancelled("cancelled")]
#[case::failed("failed")]
#[case::withdrawn("withdrawn")]
#[tokio::test]
async fn terminal_operations_only_project_the_saved_result_even_on_cancel(
	mut control: Control,
	#[case] state: &str,
) {
	control.operation.state = state.into();
	assert_eq!(
		poll(
			&mut control,
			Uuid::from_u128(13),
			None,
			"code_interpreter",
			true
		)
		.await
		.unwrap(),
		json!({"state":state,"result":{"prior":"unchanged"}})
	);
	assert_eq!(control.calls, vec!["load", "project"]);
}
#[rstest]
#[case::principal("principal")]
#[case::area("area")]
#[case::run("run")]
#[case::kind("kind")]
#[tokio::test]
async fn hidden_operations_never_project_or_cancel_foreign_rows(
	mut control: Control,
	#[case] mismatch: &str,
) {
	match mismatch {
		"principal" => control.operation.principal = "other".into(),
		"area" => control.operation.area_id = Uuid::nil(),
		"run" => control.operation.run_id = Uuid::nil(),
		"kind" => control.operation.kind = "shell".into(),
		_ => unreachable!(),
	}
	assert!(
		matches!(poll(&mut control,Uuid::from_u128(13),None,"code_interpreter",true).await,Err(Error::NotFound(message)) if message=="operation unavailable")
	);
	assert_eq!(control.calls, vec!["load"]);
}
#[rstest]
#[case::area("activate", 2)]
#[case::python("python", 3)]
#[case::persist("persist", 4)]
#[tokio::test]
async fn cancelled_result_is_not_projected_before_every_transactional_effect_succeeds(
	mut control: Control,
	#[case] failure: &'static str,
	#[case] last: usize,
) {
	control.failure = Some(failure);
	assert!(matches!(
		poll(
			&mut control,
			Uuid::from_u128(13),
			None,
			"code_interpreter",
			true
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		control.calls,
		vec!["load", "change", "activate", "python", "persist"][..=last]
	);
	assert_eq!(control.offset, None);
}
#[fixture]
fn input() -> ShellRequest {
	ShellRequest {
		idempotency_key: Uuid::from_u128(1),
		command: "printf 'ok'".into(),
		timeout_seconds: None,
		expected_revision: 7,
	}
}
#[fixture]
fn run() -> RunMetadata {
	RunMetadata {
		id: Uuid::from_u128(2),
		task_id: Uuid::from_u128(3),
		workspace_id: Uuid::from_u128(4),
		home_node: "aidash://home".into(),
		agent_id: "agent".into(),
		agent_version: "1".into(),
		phase: RunPhase::Ready,
		control: RunControl::Active,
		step: 3,
		revision: 5,
		observed_input_seq: 7,
		ledger_worker_ready: true,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: Utc::now(),
	}
}
struct Scope {
	limits: AdmissionLimits,
	area: AreaSnapshot,
	active: Option<Uuid>,
	previous: Option<(Uuid, String)>,
	files: Vec<MountedFile>,
	failure: Option<&'static str>,
	calls: Vec<String>,
	accepted: Vec<AcceptedOperation>,
}
#[fixture]
fn scope(run: RunMetadata) -> Scope {
	Scope {
		limits: AdmissionLimits {
			admission: true,
			working_bytes: 16,
			command_bytes: 16,
			operation_seconds: 10,
			maximum_seconds: 20,
		},
		area: AreaSnapshot {
			id: Uuid::from_u128(5),
			workspace_id: run.workspace_id,
			state: "active".into(),
			epoch: 9,
			revision: 7,
		},
		active: Some(run.id),
		previous: None,
		files: vec![],
		failure: None,
		calls: vec![],
		accepted: vec![],
	}
}
impl Scope {
	fn record(&mut self, stage: impl Into<String>) -> Result<()> {
		let stage = stage.into();
		self.calls.push(stage.clone());
		if self.failure.is_some_and(|failure| failure == stage) {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
}
#[async_trait]
impl OperationAdmissionScope<Uuid> for Scope {
	fn limits(&self) -> AdmissionLimits {
		self.limits
	}
	fn area(&self) -> AreaSnapshot {
		self.area.clone()
	}
	async fn previous(&mut self, key: &str) -> Result<Option<(Uuid, String)>> {
		assert_eq!(
			key,
			format!("core:{}:{}", Uuid::from_u128(2), Uuid::from_u128(1))
		);
		self.record("previous")?;
		Ok(self.previous.clone())
	}
	async fn result(&mut self, operation: &Uuid, offset: usize) -> Result<Value> {
		assert_eq!(offset, 0);
		self.record("result")?;
		Ok(json!({"operation_id":operation,"saved":true}))
	}
	async fn active_run(&mut self) -> Result<Option<Uuid>> {
		self.record("active")?;
		Ok(self.active)
	}
	async fn require_write(&mut self) -> Result<()> {
		self.record("file.write")
	}
	async fn request_files(
		&mut self,
		run: Uuid,
		mut package_files: Vec<MountedFile>,
	) -> Result<Vec<MountedFile>> {
		assert_eq!(run, Uuid::from_u128(2));
		self.record("files")?;
		let mut files = self.files.clone();
		files.append(&mut package_files);
		Ok(files)
	}
	async fn verified_health(&mut self, python: bool) -> Result<()> {
		self.record(if python {
			"health:python"
		} else {
			"health:shell"
		})
	}
	async fn release_python(&mut self, reason: &str) -> Result<()> {
		self.record(format!("release:{reason}"))
	}
	fn operation_id(&self) -> Uuid {
		Uuid::from_u128(6)
	}
	async fn set_running(&mut self, epoch: i64) -> Result<()> {
		self.record("running")?;
		assert_eq!(epoch, 10);
		self.area.epoch = epoch;
		Ok(())
	}
	async fn accept(&mut self, operation: AcceptedOperation) -> Result<Uuid> {
		self.record("accept")?;
		let id = operation.id;
		self.accepted.push(operation);
		Ok(id)
	}
}
#[rstest]
#[case::shell("shell",vec!["health:shell","release:shell_requested"])]
#[case::python("code_interpreter",vec!["health:python"])]
#[case::install("python_install",vec!["health:shell","release:packages_changed"])]
#[tokio::test]
async fn operation_admission_keeps_authorization_mount_probe_release_and_accept_order(
	mut scope: Scope,
	run: RunMetadata,
	input: ShellRequest,
	#[case] kind: &str,
	#[case] extra_stages: Vec<&str>,
) {
	let extra = json!({"session_id":Uuid::from_u128(7)});
	let digest = input.digest(kind, run.id, &extra);
	let result = prepare(&mut scope, &run, input, kind, extra).await.unwrap();
	assert_eq!(
		result,
		json!({"operation_id":Uuid::from_u128(6),"saved":true})
	);
	let mut calls = vec!["previous", "active", "file.write", "files"];
	calls.extend(extra_stages);
	calls.extend(["running", "accept", "result"]);
	assert_eq!(scope.calls, calls);
	assert_eq!(scope.accepted.len(), 1);
	let operation = &scope.accepted[0];
	assert_eq!(operation.epoch, 10);
	assert_eq!(operation.digest, digest);
	assert_eq!(
		operation.input,
		json!({"command":"printf 'ok'","seconds":10,"session_id":Uuid::from_u128(7)})
	);
}
#[rstest]
#[tokio::test]
async fn matching_idempotent_replay_precedes_current_admission_and_area_checks(
	mut scope: Scope,
	run: RunMetadata,
	input: ShellRequest,
) {
	scope.previous = Some((
		Uuid::from_u128(8),
		input.digest("shell", run.id, &json!({})),
	));
	scope.limits.admission = false;
	scope.area.state = "uncertain".into();
	scope.active = None;
	assert_eq!(
		prepare(&mut scope, &run, input, "shell", json!({}))
			.await
			.unwrap(),
		json!({"operation_id":Uuid::from_u128(8),"saved":true})
	);
	assert_eq!(scope.calls, vec!["previous", "result"]);
	assert!(scope.accepted.is_empty());
}
#[rstest]
#[tokio::test]
async fn idempotent_replay_rejects_different_request_bytes_without_projection(
	mut scope: Scope,
	run: RunMetadata,
	input: ShellRequest,
) {
	scope.previous = Some((Uuid::from_u128(8), "different".into()));
	assert!(
		matches!(prepare(&mut scope,&run,input,"shell",json!({})).await,Err(Error::Conflict(code)) if code=="IDEMPOTENCY_CONFLICT")
	);
	assert_eq!(scope.calls, vec!["previous"]);
	assert!(scope.accepted.is_empty());
}
#[rstest]
#[case::area("AREA_BUSY")]
#[case::disabled("CAPABILITIES_DISABLED")]
#[case::revision("AREA_REVISION_CHANGED")]
#[case::inactive("RUN_NOT_ACTIVE")]
#[case::terminal("TERMINAL")]
#[tokio::test]
async fn new_operation_admission_stops_before_file_authority_on_invalid_saved_state(
	mut scope: Scope,
	mut run: RunMetadata,
	input: ShellRequest,
	#[case] condition: &str,
) {
	match condition {
		"AREA_BUSY" => scope.area.state = "running".into(),
		"CAPABILITIES_DISABLED" => scope.limits.admission = false,
		"AREA_REVISION_CHANGED" => scope.area.revision = 8,
		"RUN_NOT_ACTIVE" => scope.active = None,
		"TERMINAL" => run.phase = RunPhase::Completed,
		_ => unreachable!(),
	}
	let expected = if condition == "TERMINAL" {
		"RUN_NOT_ACTIVE"
	} else {
		condition
	};
	assert_eq!(
		prepare(&mut scope, &run, input, "shell", json!({}))
			.await
			.unwrap_err()
			.to_string(),
		expected
	);
	assert!(!scope.calls.iter().any(|stage| stage == "file.write"));
	assert!(scope.accepted.is_empty());
}
#[rstest]
#[case::authorization("file.write", 2)]
#[case::mounts("files", 3)]
#[case::health("health:shell", 4)]
#[case::python_release("release:shell_requested", 5)]
#[case::mark_running("running", 6)]
#[case::accept("accept", 7)]
#[tokio::test]
async fn faults_stop_before_later_admission_effects(
	mut scope: Scope,
	run: RunMetadata,
	input: ShellRequest,
	#[case] stage: &'static str,
	#[case] last: usize,
) {
	scope.failure = Some(stage);
	assert!(matches!(
		prepare(&mut scope, &run, input, "shell", json!({})).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		scope.calls,
		vec![
			"previous",
			"active",
			"file.write",
			"files",
			"health:shell",
			"release:shell_requested",
			"running",
			"accept"
		][..=last]
	);
	assert!(scope.accepted.is_empty());
}
#[rstest]
#[tokio::test]
async fn package_mounts_cannot_consume_the_entire_writable_quota(
	mut scope: Scope,
	run: RunMetadata,
	input: ShellRequest,
) {
	let file = MountedFile {
		file_id: Uuid::from_u128(9),
		path: "wheel.whl".into(),
		digest: "digest".into(),
		size: 16,
		media_type: "application/octet-stream".into(),
		scope: FileScope::References,
		provenance: json!({}),
	};
	assert!(
		matches!(prepare(&mut scope,&run,input,"shell",json!({"package_files":[file]})).await,Err(Error::Conflict(code)) if code=="WORKING_QUOTA_EXCEEDED")
	);
	assert_eq!(
		scope.calls,
		vec!["previous", "active", "file.write", "files"]
	);
	assert!(scope.accepted.is_empty());
}
#[rstest]
#[tokio::test]
async fn invalid_extra_shape_retains_the_original_rollback_boundary_after_running_mark(
	mut scope: Scope,
	run: RunMetadata,
	input: ShellRequest,
) {
	assert!(matches!(
		prepare(&mut scope, &run, input, "shell", json!([])).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		scope.calls,
		vec![
			"previous",
			"active",
			"file.write",
			"files",
			"health:shell",
			"release:shell_requested",
			"running"
		]
	);
	assert!(scope.accepted.is_empty());
}
