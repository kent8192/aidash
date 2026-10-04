use super::*;
use aidash_domain::{
	RawRun, RunControl, RunMetadata, RunPhase,
	context::Context,
	identity::execution::{ExecutionGrant, ExecutionPrincipal},
	policy::Resource,
};
use async_trait::async_trait;
use chrono::Utc;
use rstest::{fixture, rstest};
use serde_json::Value;
use std::sync::{Arc, Mutex};

fn id(value: u128) -> Uuid {
	Uuid::from_u128(value)
}
struct State {
	raw: Option<RawRun>,
	grant: Option<ExecutionGrant>,
	principal: ExecutionPrincipal,
	visible: bool,
	fail: Option<&'static str>,
	deny: Option<&'static str>,
	pending_control: bool,
	calls: Vec<String>,
	context: Option<Value>,
	credential: Option<Uuid>,
	committed: bool,
}
struct Repository(Arc<Mutex<State>>);
struct Scope {
	state: Arc<Mutex<State>>,
	finished: bool,
}
impl State {
	fn record(&mut self, action: &str) -> Result<()> {
		self.calls.push(action.into());
		if self.deny == Some(action) {
			return Err(Error::Forbidden);
		}
		if self.fail == Some(action) {
			return Err(Error::Port(Box::new(std::io::Error::other(format!(
				"{action} fault"
			)))));
		}
		Ok(())
	}
}
impl Drop for Scope {
	fn drop(&mut self) {
		if !self.finished {
			self.state.lock().unwrap().calls.push("rollback".into());
		}
	}
}
#[fixture]
fn repository() -> Repository {
	let metadata = RunMetadata {
		id: id(1),
		task_id: id(2),
		workspace_id: id(3),
		home_node: "aidash://home".into(),
		agent_id: "producer".into(),
		agent_version: "1".into(),
		phase: RunPhase::Ready,
		control: RunControl::Paused,
		step: 2,
		revision: 7,
		observed_input_seq: 4,
		ledger_worker_ready: true,
		error: Some("paused reason".into()),
		lease_owner: None,
		lease_until: None,
		updated_at: Utc::now(),
	};
	Repository(Arc::new(Mutex::new(State {
		raw: Some(RawRun {
			metadata,
			context: serde_json::to_value(Context::default()).unwrap(),
			pending: json!({"state_version":1,"data":{},"recovery":{"retry":null,"lease_recovered":false}}),
		}),
		grant: Some(ExecutionGrant {
			run_id: id(1),
			task_id: id(2),
			workspace_id: id(3),
			tenant: "tenant".into(),
			credential_id: id(8),
			root_subject: "operator".into(),
			subject_chain: vec!["operator".into()],
		}),
		principal: ExecutionPrincipal {
			tenant: "tenant".into(),
			subject: "operator".into(),
			credential_id: id(9),
		},
		visible: true,
		fail: None,
		deny: None,
		pending_control: false,
		calls: vec![],
		context: None,
		credential: None,
		committed: false,
	})))
}
#[async_trait]
impl RunControlRepository for Repository {
	async fn begin(&self) -> Result<Box<dyn RunControlScope + '_>> {
		self.0.lock().unwrap().record("begin")?;
		Ok(Box::new(Scope {
			state: self.0.clone(),
			finished: false,
		}))
	}
	fn notify(&self) {
		let mut state = self.0.lock().unwrap();
		assert!(
			state.committed,
			"workers must not observe an uncommitted control"
		);
		state.calls.push("notify".into());
	}
}
#[async_trait]
impl RunControlScope for Scope {
	fn identity(&self) -> ExecutionPrincipal {
		self.state.lock().unwrap().principal.clone()
	}
	fn set_context(&mut self, context: Value) {
		self.state.lock().unwrap().context = Some(context);
	}
	fn resource(&self, kind: &str, key: &str, attributes: Value) -> Resource {
		Resource {
			tenant: "tenant".into(),
			kind: kind.into(),
			id: key.into(),
			attributes,
		}
	}
	async fn run(&mut self, key: Uuid) -> Result<Option<RawRun>> {
		assert_eq!(key, id(1));
		let mut state = self.state.lock().unwrap();
		state.record("run")?;
		Ok(state.raw.clone())
	}
	async fn workspace(&mut self, key: Uuid) -> Result<Resource> {
		assert_eq!(key, id(3));
		self.state.lock().unwrap().record("workspace")?;
		Ok(self.resource(
			"workspace",
			&key.to_string(),
			json!({"current":"workspace attributes"}),
		))
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		let mut state = self.state.lock().unwrap();
		assert_eq!(
			state.context,
			Some(json!({"current":"workspace attributes"}))
		);
		assert_eq!(resource.tenant, state.principal.tenant);
		if action == "run.control" {
			assert_eq!(resource.kind, "run");
			assert_eq!(resource.id, id(1).to_string());
			assert_eq!(resource.attributes, json!({}));
		}
		state.record(action)
	}
	async fn run_visible(&mut self, run: &RunInspection) -> Result<bool> {
		assert_eq!(run.id, id(1));
		let mut state = self.state.lock().unwrap();
		state.record("visible")?;
		Ok(state.visible)
	}
	async fn lock_execution_grant(&mut self, key: Uuid) -> Result<Option<ExecutionGrant>> {
		assert_eq!(key, id(1));
		let mut state = self.state.lock().unwrap();
		state.record("grant_lock")?;
		Ok(state.grant.clone())
	}
	async fn update_credential(&mut self, key: Uuid, credential: Uuid) -> Result<()> {
		assert_eq!(key, id(1));
		let mut state = self.state.lock().unwrap();
		state.record("credential")?;
		state.credential = Some(credential);
		Ok(())
	}
	async fn control_run(&mut self, key: Uuid, action: RunControlAction) -> Result<RunInspection> {
		assert_eq!(key, id(1));
		let (pending, mut run) = {
			let mut state = self.state.lock().unwrap();
			state.record("control")?;
			(state.pending_control, state.raw.clone().unwrap().inspect())
		};
		if pending {
			std::future::pending::<()>().await;
		}
		run.metadata.control = action.control();
		run.metadata.revision += 1;
		Ok(run)
	}
	async fn finish(mut self: Box<Self>, result: Result<RunInspection>) -> Result<RunInspection> {
		match result {
			Ok(run) => {
				self.state.lock().unwrap().record("commit")?;
				self.state.lock().unwrap().committed = true;
				self.finished = true;
				Ok(run)
			}
			Err(error) => {
				self.state.lock().unwrap().calls.push("rollback".into());
				self.finished = true;
				Err(error)
			}
		}
	}
}
fn expected(resume: bool) -> Vec<&'static str> {
	let mut calls = vec![
		"begin",
		"run",
		"workspace",
		"workspace.read",
		"visible",
		"run.control",
	];
	if resume {
		calls.extend(["grant_lock", "credential"]);
	}
	calls.extend(["control", "commit", "notify"]);
	calls
}
#[rstest]
#[case::pause(RunControlAction::Pause)]
#[case::cancel(RunControlAction::Cancel)]
#[case::resume(RunControlAction::Resume)]
#[tokio::test]
async fn an_authorized_control_commits_before_waking_workers(
	repository: Repository,
	#[case] action: RunControlAction,
) {
	let run = control(&repository, id(1), action).await.unwrap();
	assert_eq!(run.control, action.control());
	assert_eq!(run.revision, 8);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.calls, expected(action == RunControlAction::Resume));
	assert_eq!(
		state.credential,
		if action == RunControlAction::Resume {
			Some(id(9))
		} else {
			None
		}
	);
}
#[rstest]
#[case::missing_run("run")]
#[case::invisible_run("visible")]
#[case::missing_grant("grant_lock")]
#[case::different_tenant("tenant")]
#[case::different_root_subject("subject")]
#[tokio::test]
async fn undisclosable_or_unowned_runs_do_not_refresh_credentials_or_mutate(
	repository: Repository,
	#[case] boundary: &str,
) {
	{
		let mut state = repository.0.lock().unwrap();
		match boundary {
			"run" => state.raw = None,
			"visible" => state.visible = false,
			"grant_lock" => state.grant = None,
			"tenant" => state.grant.as_mut().unwrap().tenant = "other".into(),
			"subject" => state.grant.as_mut().unwrap().root_subject = "other".into(),
			_ => panic!("unexpected boundary"),
		}
	}
	assert!(matches!(
		control(&repository, id(1), RunControlAction::Resume).await,
		Err(Error::Forbidden)
	));
	let state = repository.0.lock().unwrap();
	assert!(!state.committed);
	assert_eq!(state.credential, None);
	assert!(!state.calls.contains(&"control".into()));
	assert!(!state.calls.contains(&"notify".into()));
	assert_eq!(state.calls.last().map(String::as_str), Some("rollback"));
}
#[rstest]
#[case::workspace("workspace.read")]
#[case::run("run.control")]
#[tokio::test]
async fn revocation_prevents_control_before_the_resume_grant_lock(
	repository: Repository,
	#[case] action: &'static str,
) {
	repository.0.lock().unwrap().deny = Some(action);
	assert!(matches!(
		control(&repository, id(1), RunControlAction::Resume).await,
		Err(Error::Forbidden)
	));
	let state = repository.0.lock().unwrap();
	assert!(!state.calls.contains(&"grant_lock".into()));
	assert!(!state.committed);
	assert_eq!(state.calls.last().map(String::as_str), Some("rollback"));
}
#[rstest]
#[case::begin("begin")]
#[case::run("run")]
#[case::workspace("workspace")]
#[case::workspace_authority("workspace.read")]
#[case::visibility("visible")]
#[case::control_authority("run.control")]
#[case::grant_lock("grant_lock")]
#[case::credential_update("credential")]
#[case::atomic_control("control")]
#[case::commit("commit")]
#[tokio::test]
async fn adapter_faults_keep_their_identity_and_never_wake_workers(
	repository: Repository,
	#[case] boundary: &'static str,
) {
	repository.0.lock().unwrap().fail = Some(boundary);
	let Error::Port(error) = control(&repository, id(1), RunControlAction::Resume)
		.await
		.err()
		.unwrap()
	else {
		panic!("expected adapter fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		format!("{boundary} fault")
	);
	let state = repository.0.lock().unwrap();
	assert!(!state.committed);
	assert!(!state.calls.contains(&"notify".into()));
	let expected = expected(true);
	let last = expected.iter().position(|s| *s == boundary).unwrap();
	assert_eq!(&state.calls[..=last], &expected[..=last]);
	assert_eq!(
		state.calls.len(),
		last + 1 + usize::from(boundary != "begin")
	);
}
#[rstest]
#[tokio::test]
async fn cancelling_a_control_future_releases_the_scope_without_commit_or_notification(
	repository: Repository,
) {
	repository.0.lock().unwrap().pending_control = true;
	assert!(
		tokio::time::timeout(
			std::time::Duration::from_millis(10),
			control(&repository, id(1), RunControlAction::Resume)
		)
		.await
		.is_err()
	);
	let state = repository.0.lock().unwrap();
	assert!(!state.committed);
	assert_eq!(state.calls.last().map(String::as_str), Some("rollback"));
	assert!(!state.calls.contains(&"notify".into()));
}
#[rstest]
#[tokio::test]
async fn cancelling_an_inspectable_corrupt_run_does_not_turn_its_context_into_executable_state(
	repository: Repository,
) {
	repository.0.lock().unwrap().raw.as_mut().unwrap().context = Value::Null;
	let result = control(&repository, id(1), RunControlAction::Cancel)
		.await
		.unwrap();
	assert_eq!(result.control, RunControl::Cancelled);
	assert!(result.context.is_none());
	assert_eq!(
		result.state_error.as_deref(),
		Some("invalid execution context")
	);
	assert_eq!(repository.0.lock().unwrap().calls, expected(false));
}
