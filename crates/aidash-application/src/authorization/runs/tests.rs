use super::*;
use aidash_domain::{
	RawRun, RunControl, RunMetadata, RunPhase,
	context::Context,
	identity::execution::{ExecutionGrant, ExecutionPrincipal},
	policy::Resource,
	registry::EntityRef,
	run_state::RunReadsBlock,
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
	block: Option<RunReadsBlock>,
	/// What the Run's reads say against it, apart from whether the reader may read the Run.
	stop: Option<RunReadsBlock>,
	use_block: Option<RunReadsBlock>,
	fail: Option<&'static str>,
	deny: Option<&'static str>,
	pending_control: bool,
	calls: Vec<String>,
	context: Option<Value>,
	credential: Option<Uuid>,
	committed: bool,
	declined: Option<String>,
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
		node: "brave-otter-k3m9q2xa".parse().unwrap(),
		id: id(1),
		task_id: id(2),
		workspace_id: id(3),
		home_node: "aidash://home/brave-otter-k3m9q2xa".into(),
		agent_id: "producer".into(),
		agent_version: "1".into(),
		phase: RunPhase::Ready,
		control: RunControl::Paused,
		step: 2,
		revision: 7,
		observed_input_seq: 4,
		ledger_worker_ready: true,
		control_revision: 0,
		control_source: Default::default(),
		error: Some("paused reason".into()),
		lease_owner: None,
		lease_until: None,
		updated_at: Utc::now(),
	};
	Repository(Arc::new(Mutex::new(State {
		raw: Some(RawRun {
			metadata,
			context: serde_json::to_value(Context::default()).unwrap(),
			pending: json!({"state_version":1,"data":{},"recovery":{"retry":null}}),
		}),
		grant: Some(ExecutionGrant {
			run_id: id(1),
			task_id: id(2),
			workspace_id: id(3),
			tenant: "tenant".into(),
			credential_id: id(8),
			root_subject: "deployment_administrator".into(),
			subject_chain: vec!["deployment_administrator".into()],
		}),
		principal: ExecutionPrincipal {
			tenant: "tenant".into(),
			subject: "deployment_administrator".into(),
			credential_id: id(9),
		},
		visible: true,
		block: None,
		stop: None,
		use_block: None,
		fail: None,
		deny: None,
		pending_control: false,
		calls: vec![],
		context: None,
		credential: None,
		committed: false,
		declined: None,
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
	fn node_id(&self) -> &str {
		"aidash://home"
	}
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
	async fn run_reads_block(&mut self, run: &RunInspection) -> Result<Option<RunReadsBlock>> {
		assert_eq!(run.id, id(1));
		let mut state = self.state.lock().unwrap();
		state.record("block")?;
		Ok(state.block.clone())
	}
	async fn run_reads_stop(&mut self, run: &RunInspection) -> Result<Option<RunReadsBlock>> {
		assert_eq!(run.id, id(1));
		let mut state = self.state.lock().unwrap();
		state.record("stop")?;
		Ok(state.stop.clone())
	}
	async fn use_block(&mut self, run: &RunInspection) -> Result<Option<RunReadsBlock>> {
		assert_eq!(run.id, id(1));
		let mut state = self.state.lock().unwrap();
		state.record("use")?;
		Ok(state.use_block.clone())
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
	async fn resume_run(&mut self, key: Uuid) -> Result<RunInspection> {
		assert_eq!(key, id(1));
		let (pending, mut run) = {
			let mut state = self.state.lock().unwrap();
			state.record("control")?;
			(state.pending_control, state.raw.clone().unwrap().inspect())
		};
		if pending {
			std::future::pending::<()>().await;
		}
		run.metadata.control = RunControl::Active;
		run.metadata.revision += 1;
		Ok(run)
	}
	async fn decline_call(&mut self, key: Uuid, call: &str) -> Result<RunInspection> {
		assert_eq!(key, id(1));
		let mut run = {
			let mut state = self.state.lock().unwrap();
			state.record("decline")?;
			state.declined = Some(call.to_owned());
			state.raw.clone().unwrap().inspect()
		};
		run.metadata.control = RunControl::Active;
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
fn expected() -> Vec<&'static str> {
	vec![
		"begin",
		"run",
		"workspace",
		"workspace.read",
		"visible",
		"run.control",
		"grant_lock",
		"use",
		"credential",
		"control",
		"commit",
		"notify",
	]
}
#[rstest]
#[tokio::test]
async fn an_authorized_resume_commits_before_waking_workers(repository: Repository) {
	let run = resume(&repository, id(1)).await.unwrap();
	assert_eq!(run.control, RunControl::Active);
	assert_eq!(run.revision, 8);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.calls, expected());
	assert_eq!(state.credential, Some(id(9)));
}
fn withdrawn() -> RunReadsBlock {
	RunReadsBlock::DefinitionNotApproved {
		definition: Some(EntityRef {
			id: "qa".into(),
			version: "2.1.0".into(),
		}),
	}
}
#[rstest]
#[tokio::test]
async fn resume_says_what_blocks_it_and_changes_nothing(repository: Repository) {
	{
		let mut state = repository.0.lock().unwrap();
		state.visible = false;
		state.block = Some(withdrawn());
	}
	let result = resume(&repository, id(1)).await;
	assert!(matches!(result, Err(Error::ResumeBlocked(block)) if block == withdrawn()));
	let state = repository.0.lock().unwrap();
	assert!(!state.committed);
	assert_eq!(state.credential, None);
	assert!(!state.calls.contains(&"control".into()));
	assert!(!state.calls.contains(&"notify".into()));
	assert_eq!(state.calls.last().map(String::as_str), Some("rollback"));
}
fn invalidated(state: &mut State) {
	state.raw.as_mut().unwrap().pending["recovery"]["semantic_reason"] = json!("invalidated");
	// Its disclosure is withheld for the same change, and the read walk has
	// nothing more to say about it.
	state.visible = false;
}
#[rstest]
#[tokio::test]
async fn an_invalidated_run_is_refused_with_its_cause_to_the_person_who_could_resume_it(
	repository: Repository,
) {
	invalidated(&mut repository.0.lock().unwrap());
	let result = resume(&repository, id(1)).await;
	assert!(
		matches!(
			result,
			Err(Error::ResumeBlocked(RunReadsBlock::SourceChanged))
		),
		"{result:?}"
	);
	let state = repository.0.lock().unwrap();
	assert!(!state.committed);
	assert_eq!(state.credential, None);
	assert!(!state.calls.contains(&"visible".into()));
	assert!(!state.calls.contains(&"control".into()));
	assert!(!state.calls.contains(&"notify".into()));
}
#[rstest]
#[case::no_control("run.control")]
#[case::not_the_root("subject")]
#[tokio::test]
async fn someone_who_may_not_resume_an_invalidated_run_never_learns_why(
	repository: Repository,
	#[case] boundary: &'static str,
) {
	{
		let mut state = repository.0.lock().unwrap();
		invalidated(&mut state);
		match boundary {
			"run.control" => state.deny = Some("run.control"),
			"subject" => state.grant.as_mut().unwrap().root_subject = "other".into(),
			_ => panic!("unexpected boundary"),
		}
	}
	assert!(matches!(
		resume(&repository, id(1)).await,
		Err(Error::Forbidden)
	));
}
#[rstest]
#[tokio::test]
async fn another_semantic_pause_is_still_resumed_through_the_read_walk(repository: Repository) {
	repository.0.lock().unwrap().raw.as_mut().unwrap().pending["recovery"]["semantic_reason"] =
		json!("retries_exhausted");
	let run = resume(&repository, id(1)).await.unwrap();
	assert_eq!(run.control, RunControl::Active);
	assert_eq!(repository.0.lock().unwrap().calls, expected());
}
#[rstest]
#[case::completed(RunPhase::Completed, RunControl::Active)]
#[case::failed(RunPhase::Failed, RunControl::Active)]
#[case::cancelled(RunPhase::Cancelled, RunControl::Active)]
#[case::cancelling(RunPhase::Thinking, RunControl::Cancelled)]
#[tokio::test]
async fn resume_of_an_ended_run_is_answered_by_the_resume_not_the_use_recheck(
	repository: Repository,
	#[case] phase: RunPhase,
	#[case] control: RunControl,
) {
	{
		let mut state = repository.0.lock().unwrap();
		state.use_block = Some(unapproved());
		let raw = state.raw.as_mut().unwrap();
		raw.metadata.phase = phase;
		raw.metadata.control = control;
	}
	let result = resume(&repository, id(1)).await;
	assert!(!matches!(result, Err(Error::ResumeBlocked(_))));
	let state = repository.0.lock().unwrap();
	assert!(!state.calls.contains(&"use".into()));
	assert!(state.calls.contains(&"control".into()));
}
#[rstest]
#[case::no_control("run.control")]
#[case::no_grant("grant")]
#[case::not_the_root("subject")]
#[tokio::test]
async fn someone_who_may_not_resume_never_learns_what_blocks_it(
	repository: Repository,
	#[case] boundary: &'static str,
) {
	{
		let mut state = repository.0.lock().unwrap();
		state.visible = false;
		state.block = Some(withdrawn());
		match boundary {
			"run.control" => state.deny = Some("run.control"),
			"grant" => state.grant = None,
			"subject" => state.grant.as_mut().unwrap().root_subject = "other".into(),
			_ => panic!("unexpected boundary"),
		}
	}
	assert!(matches!(
		resume(&repository, id(1)).await,
		Err(Error::Forbidden)
	));
}
#[rstest]
#[tokio::test]
async fn a_failed_authorize_never_reports_a_block(repository: Repository) {
	// Live preview's check falls back on `Forbidden` (ADR 0070).
	{
		let mut state = repository.0.lock().unwrap();
		state.visible = false;
		state.block = Some(withdrawn());
	}
	assert!(matches!(
		authorize_run(&repository, id(1)).await,
		Err(Error::Forbidden)
	));
	assert!(!repository.0.lock().unwrap().calls.contains(&"block".into()));
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
		resume(&repository, id(1)).await,
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
		resume(&repository, id(1)).await,
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
#[case::use_recheck("use")]
#[case::credential_update("credential")]
#[case::atomic_control("control")]
#[case::commit("commit")]
#[tokio::test]
async fn adapter_faults_keep_their_identity_and_never_wake_workers(
	repository: Repository,
	#[case] boundary: &'static str,
) {
	repository.0.lock().unwrap().fail = Some(boundary);
	let Error::Port(error) = resume(&repository, id(1)).await.err().unwrap() else {
		panic!("expected adapter fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		format!("{boundary} fault")
	);
	let state = repository.0.lock().unwrap();
	assert!(!state.committed);
	assert!(!state.calls.contains(&"notify".into()));
	let expected = expected();
	let last = expected.iter().position(|s| *s == boundary).unwrap();
	assert_eq!(&state.calls[..=last], &expected[..=last]);
	assert_eq!(
		state.calls.len(),
		last + 1 + usize::from(boundary != "begin")
	);
}
#[rstest]
#[tokio::test]
async fn cancelling_a_resume_future_releases_the_scope_without_commit_or_notification(
	repository: Repository,
) {
	repository.0.lock().unwrap().pending_control = true;
	assert!(
		tokio::time::timeout(
			std::time::Duration::from_millis(10),
			resume(&repository, id(1))
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
async fn resuming_an_inspectable_corrupt_run_does_not_turn_its_context_into_executable_state(
	repository: Repository,
) {
	repository.0.lock().unwrap().raw.as_mut().unwrap().context = Value::Null;
	let result = resume(&repository, id(1)).await.unwrap();
	assert_eq!(result.control, RunControl::Active);
	assert!(result.context.is_none());
	assert_eq!(
		result.state_error.as_deref(),
		Some("invalid execution context")
	);
	assert_eq!(repository.0.lock().unwrap().calls, expected());
}

fn call() -> &'static str {
	"run:7:0"
}
fn expected_decline() -> Vec<&'static str> {
	vec![
		"begin",
		"run",
		"workspace",
		"workspace.read",
		"grant_lock",
		"run.control",
		"visible",
		"use",
		"credential",
		"decline",
		"commit",
		"notify",
	]
}
#[rstest]
#[tokio::test]
async fn the_runs_person_declines_under_run_control_and_takes_the_credential(
	repository: Repository,
) {
	let run = decline_call(&repository, id(1), call()).await.unwrap();
	assert_eq!(run.control, RunControl::Active);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.calls, expected_decline());
	assert_eq!(state.credential, Some(id(9)));
	assert_eq!(state.declined.as_deref(), Some(call()));
}
#[rstest]
#[tokio::test]
async fn a_holder_of_run_decline_call_declines_and_leaves_the_credential(repository: Repository) {
	repository
		.0
		.lock()
		.unwrap()
		.grant
		.as_mut()
		.unwrap()
		.root_subject = "other".into();
	decline_call(&repository, id(1), call()).await.unwrap();
	let state = repository.0.lock().unwrap();
	assert!(state.calls.contains(&"run.decline_call".into()));
	assert!(!state.calls.contains(&"run.control".into()));
	assert!(!state.calls.contains(&"credential".into()));
	assert_eq!(state.credential, None);
	assert_eq!(state.declined.as_deref(), Some(call()));
}
#[rstest]
#[tokio::test]
async fn someone_else_declines_a_run_they_cannot_read_by_its_read_walk_alone(
	repository: Repository,
) {
	{
		let mut state = repository.0.lock().unwrap();
		state.grant.as_mut().unwrap().root_subject = "other".into();
		// The Run is private to its person, as a Run with a Working Area is.
		state.visible = false;
	}
	decline_call(&repository, id(1), call()).await.unwrap();
	let state = repository.0.lock().unwrap();
	assert!(!state.calls.contains(&"visible".into()));
	assert!(!state.calls.contains(&"block".into()));
	assert!(state.calls.contains(&"stop".into()));
	assert_eq!(state.declined.as_deref(), Some(call()));
}
#[rstest]
#[tokio::test]
async fn what_the_run_read_still_stops_someone_else_with_the_same_reason(repository: Repository) {
	{
		let mut state = repository.0.lock().unwrap();
		state.grant.as_mut().unwrap().root_subject = "other".into();
		state.stop = Some(withdrawn());
	}
	let result = decline_call(&repository, id(1), call()).await;
	assert!(matches!(result, Err(Error::ResumeBlocked(block)) if block == withdrawn()));
	let state = repository.0.lock().unwrap();
	assert!(state.declined.is_none());
	assert_eq!(state.credential, None);
	assert!(!state.committed);
}
#[rstest]
#[case::another_person_with_only_run_control("other", "run.decline_call")]
#[case::the_runs_person_without_run_control("deployment_administrator", "run.control")]
#[tokio::test]
async fn nobody_declines_without_the_authority_for_their_relation(
	repository: Repository,
	#[case] root: &str,
	#[case] denied: &'static str,
) {
	{
		let mut state = repository.0.lock().unwrap();
		state.grant.as_mut().unwrap().root_subject = root.into();
		state.deny = Some(denied);
	}
	assert!(matches!(
		decline_call(&repository, id(1), call()).await,
		Err(Error::Forbidden)
	));
	let state = repository.0.lock().unwrap();
	assert!(state.declined.is_none());
	assert!(!state.committed);
}
#[rstest]
#[tokio::test]
async fn a_run_another_node_executes_is_never_declined_here(repository: Repository) {
	{
		let mut state = repository.0.lock().unwrap();
		state.raw.as_mut().unwrap().metadata.home_node =
			"aidash://elsewhere/brave-otter-k3m9q2xa".into();
	}
	assert!(matches!(
		decline_call(&repository, id(1), call()).await,
		Err(Error::Forbidden)
	));
	let state = repository.0.lock().unwrap();
	assert!(state.declined.is_none());
	// Whatever the caller holds, the check ends before asking who they are.
	assert!(!state.calls.contains(&"grant_lock".into()));
}
#[rstest]
#[tokio::test]
async fn declining_says_what_blocks_it_only_to_someone_who_may_decline(repository: Repository) {
	{
		let mut state = repository.0.lock().unwrap();
		state.visible = false;
		state.block = Some(withdrawn());
	}
	let result = decline_call(&repository, id(1), call()).await;
	assert!(matches!(result, Err(Error::ResumeBlocked(block)) if block == withdrawn()));
	{
		let state = repository.0.lock().unwrap();
		assert!(state.declined.is_none());
		assert_eq!(state.credential, None);
	}
	// A holder of neither right never learns it.
	repository.0.lock().unwrap().deny = Some("run.control");
	assert!(matches!(
		decline_call(&repository, id(1), call()).await,
		Err(Error::Forbidden)
	));
}
#[rstest]
#[case::empty("")]
#[case::too_long(&"x".repeat(257))]
#[tokio::test]
async fn the_call_key_is_bounded(repository: Repository, #[case] key: &str) {
	assert!(matches!(
		decline_call(&repository, id(1), key).await,
		Err(Error::Invalid(_))
	));
	assert_eq!(
		repository.0.lock().unwrap().calls,
		vec!["begin", "rollback"]
	);
}
fn unapproved() -> RunReadsBlock {
	RunReadsBlock::DefinitionNotApproved {
		definition: Some(EntityRef {
			id: "qa".into(),
			version: "2.1.0".into(),
		}),
	}
}
#[rstest]
#[case::named(unapproved())]
#[case::unnamed(RunReadsBlock::DefinitionNotApproved { definition: None })]
#[case::policy(RunReadsBlock::DefinitionNotUsable)]
#[tokio::test]
async fn resume_of_a_run_that_cannot_use_its_binding_says_so_and_wakes_nothing(
	repository: Repository,
	#[case] block: RunReadsBlock,
) {
	repository.0.lock().unwrap().use_block = Some(block.clone());
	let result = resume(&repository, id(1)).await;
	assert!(matches!(result, Err(Error::ResumeBlocked(found)) if found == block));
	let state = repository.0.lock().unwrap();
	assert!(!state.committed);
	assert_eq!(state.credential, None);
	assert!(!state.calls.contains(&"credential".into()));
	assert!(!state.calls.contains(&"control".into()));
	assert!(!state.calls.contains(&"notify".into()));
	assert_eq!(state.calls.last().map(String::as_str), Some("rollback"));
}
#[rstest]
#[case::no_control("run.control")]
#[case::no_grant("grant")]
#[case::not_the_root("subject")]
#[tokio::test]
async fn the_use_recheck_runs_only_for_someone_who_may_resume(
	repository: Repository,
	#[case] boundary: &'static str,
) {
	{
		let mut state = repository.0.lock().unwrap();
		state.use_block = Some(unapproved());
		match boundary {
			"run.control" => state.deny = Some("run.control"),
			"grant" => state.grant = None,
			"subject" => state.grant.as_mut().unwrap().root_subject = "other".into(),
			_ => panic!("unexpected boundary"),
		}
	}
	assert!(matches!(
		resume(&repository, id(1)).await,
		Err(Error::Forbidden)
	));
	assert!(!repository.0.lock().unwrap().calls.contains(&"use".into()));
}
#[rstest]
#[tokio::test]
async fn a_read_block_is_reported_before_the_use_recheck_runs(repository: Repository) {
	{
		let mut state = repository.0.lock().unwrap();
		state.visible = false;
		state.block = Some(RunReadsBlock::DefinitionNotReadable);
		state.use_block = Some(unapproved());
	}
	assert!(matches!(
		resume(&repository, id(1)).await,
		Err(Error::ResumeBlocked(RunReadsBlock::DefinitionNotReadable))
	));
	assert!(!repository.0.lock().unwrap().calls.contains(&"use".into()));
}
