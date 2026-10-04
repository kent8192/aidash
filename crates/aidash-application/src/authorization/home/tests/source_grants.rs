//! Source use cases reuse audited fake Home transactions while exercising independent actor state.
use super::*;
use crate::{
	authorization::source::grants as use_case,
	ports::authorization::source::{
		SourceAuthorityScope, SourcePeerScope,
		grants::{GrantRepository, GrantScope},
	},
};
use aidash_domain::{
	federation::Peer,
	policy::{PolicyBundle, SubjectKind},
	registry::DefinitionValidation,
};
use std::sync::atomic::{AtomicUsize, Ordering};
struct SourceRepository {
	repository: Repository,
	task: Task,
	subjects: Vec<String>,
	bundle: PolicyBundle,
	current: Option<Grant>,
	live: bool,
	races: Arc<AtomicUsize>,
	saved_unauthorized: bool,
	hidden_reads: bool,
}
struct SourceSession {
	repository: Repository,
	task: Task,
	subjects: Vec<String>,
	bundle: PolicyBundle,
	current: Option<Grant>,
	live: bool,
	races: Arc<AtomicUsize>,
	hidden_reads: bool,
}
impl Drop for SourceSession {
	fn drop(&mut self) {
		self.repository.state.lock().unwrap().scopes -= 1;
	}
}
impl SourceRepository {
	fn new() -> Self {
		let executor = qualified_agent("aidash://receiver", "agent", "1");
		let mut current = grant(false);
		current.subject_chain.push(executor.clone());
		Self {
			repository: Repository::default(),
			task: task(),
			subjects: vec!["requester".into()],
			bundle: serde_json::from_value(
				json!({"tenant":"tenant","subjects":{executor:{"kind":"agent","delegated_by":null}}}),
			)
			.unwrap(),
			current: Some(current),
			live: true,
			races: Arc::new(AtomicUsize::new(0)),
			saved_unauthorized: false,
			hidden_reads: false,
		}
	}
	fn scope(&self) -> SourceSession {
		self.repository.state.lock().unwrap().scopes += 1;
		SourceSession {
			repository: self.repository.clone(),
			task: self.task.clone(),
			subjects: self.subjects.clone(),
			bundle: self.bundle.clone(),
			current: self.current.clone(),
			live: self.live,
			races: self.races.clone(),
			hidden_reads: self.hidden_reads,
		}
	}
}
#[async_trait]
impl GrantRepository for SourceRepository {
	type Scope = SourceSession;
	fn source_node_id(&self) -> &str {
		"aidash://home"
	}
	fn source_identity(&self) -> Option<ExecutionPrincipal> {
		HomeRepository::identity(&self.repository)
	}
	fn protocol_version(&self) -> &str {
		"current"
	}
	fn validation(&self) -> crate::registry::DefinitionValidation {
		panic!("early authority regressions must not reach inspection validation")
	}
	async fn source_begin(&self) -> Result<SourceSession> {
		self.repository.call("source_begin");
		Ok(self.scope())
	}
	async fn source_grant(&self, _: Uuid, _: &str) -> Result<Option<Grant>> {
		self.repository.call("source_grant");
		Ok(self.current.clone())
	}
	async fn begin_grant(&self, _: &Grant) -> Result<SourceSession> {
		self.repository.call("begin_saved");
		if self.saved_unauthorized {
			Err(Error::Unauthorized)
		} else {
			Ok(self.scope())
		}
	}
	async fn source_request<T: DeserializeOwned + Send>(
		&self,
		_: &str,
		_: &str,
		_: &Value,
	) -> Result<T> {
		self.repository.call("source_rpc");
		Err(Error::External("receiver unavailable".into()))
	}
}
#[async_trait]
impl HomeScope for SourceSession {
	fn identity(&self) -> ExecutionPrincipal {
		principal()
	}
	fn resource(&self, kind: &str, id: Uuid, attributes: Value) -> Resource {
		Resource {
			tenant: "tenant".into(),
			kind: kind.into(),
			id: id.to_string(),
			attributes,
		}
	}
	async fn task_read(&mut self, _: Uuid) -> Result<Task> {
		self.repository.call("task.read");
		Ok(self.task.clone())
	}
	async fn control_task(&mut self, _: Uuid) -> Result<Option<Task>> {
		self.repository.call("task.control_projection");
		Ok(Some(task()))
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		Ok(self.resource("task", task.id, json!({})))
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		Ok(self.resource("workspace", id, json!({})))
	}
	async fn require(&mut self, _: &Resource, action: &str) -> Result<()> {
		self.repository.call(action);
		if self.repository.state.lock().unwrap().denied == Some(action) {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn requester_grant(&mut self, _: Uuid, _: Uuid) -> Result<Option<Grant>> {
		Ok(Some(grant(self.repository.state.lock().unwrap().semantic)))
	}
	async fn binding(&mut self, _: Uuid) -> Result<Option<HomeBinding>> {
		let changed = self.repository.state.lock().unwrap().binding_changed;
		Ok(Some(HomeBinding {
			grant_id: Uuid::from_u128(4),
			admission_id: Uuid::from_u128(5),
			task_id: task().id,
			task_revision: 7,
			initial_task: if changed { json!({}) } else { json!(task()) },
		}))
	}
	async fn grants(&mut self, _: Uuid) -> Result<Vec<Grant>> {
		let state = self.repository.state.lock().unwrap();
		Ok((0..state.grants.max(1))
			.map(|n| {
				let mut g = grant(state.semantic);
				g.id = Uuid::from_u128(4 + n as u128);
				g
			})
			.collect())
	}
	async fn delegation_grants(&mut self, _: &Task, _: &str) -> Result<Vec<Grant>> {
		Ok(vec![grant(false)])
	}
	async fn insert_binding(
		&mut self,
		_: Uuid,
		_: Uuid,
		_: &Admission,
		_: &Description,
	) -> Result<()> {
		self.repository.call("insert_binding");
		Ok(())
	}
	async fn revoke(&mut self, _: Uuid) -> Result<()> {
		self.repository.call("revoke");
		Ok(())
	}
	async fn delivered_inputs(&mut self, _: Uuid, _: Uuid) -> Result<Vec<String>> {
		self.repository.call("delivered_inputs");
		Ok(vec!["delivered-key".into()])
	}
	async fn cancel_task(
		&mut self,
		_: Uuid,
		revision: i64,
		owner: &str,
		_: Uuid,
		keys: &[String],
	) -> Result<Task> {
		self.repository.call("cancel_task");
		self.repository.state.lock().unwrap().cancelled =
			Some((revision, owner.into(), keys.to_vec()));
		let mut task = task();
		task.revision += 1;
		task.status = TaskStatus::Cancelled;
		Ok(task)
	}
	async fn binding_revision(&mut self, _: Uuid, revision: i64) -> Result<()> {
		self.repository.state.lock().unwrap().revision = Some(revision);
		Ok(())
	}
	async fn resume_semantic(&mut self, _: Uuid, _: Uuid, _: Uuid, _: &str) -> Result<()> {
		self.repository.call("resume_semantic");
		Ok(())
	}
	async fn remote_semantic_sources(&mut self, _: Uuid) -> Result<()> {
		self.repository.call("semantic_sources");
		Ok(())
	}
	async fn grant_output_visible(&mut self, _: Uuid) -> Result<bool> {
		Ok(!self.repository.state.lock().unwrap().output_hidden)
	}
	async fn receipt(&mut self, _: Uuid) -> Result<Option<Value>> {
		self.repository.call("receipt");
		Ok(None)
	}
	async fn provenance(&mut self, _: Option<Value>) -> Result<Option<Provenance>> {
		self.repository.call("provenance");
		Ok(None)
	}
	async fn create_task(
		&mut self,
		_: Uuid,
		input: &NewTask,
		subject: &str,
		key: &str,
	) -> Result<Task> {
		self.repository.state.lock().unwrap().created =
			Some((input.clone(), subject.into(), key.into()));
		Ok(task())
	}
	async fn finish(self, result: Result<()>) -> Result<()> {
		self.repository.call(if result.is_ok() {
			"commit"
		} else {
			"rollback_with_denial_audit"
		});
		result
	}
}
#[async_trait]
impl SourceAuthorityScope for SourceSession {
	fn source_subjects(&self) -> &[String] {
		&self.subjects
	}
	fn source_bundle(&self) -> &PolicyBundle {
		&self.bundle
	}
	fn source_context(&mut self, _: Value) {}
	fn source_resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		Resource {
			tenant: "tenant".into(),
			kind: kind.into(),
			id: id.into(),
			attributes,
		}
	}
	async fn generation_home(&mut self, _: &Task, _: &str, _: Option<&Value>) -> Result<()> {
		Ok(())
	}
	async fn source_workspace(&mut self, id: Uuid) -> Result<Resource> {
		HomeScope::workspace(self, id).await
	}
	async fn source_task_resource(&mut self, task: &Task) -> Result<Resource> {
		HomeScope::task_resource(self, task).await
	}
	async fn source_require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		HomeScope::require(self, resource, action).await
	}
}
#[async_trait]
impl SourcePeerScope for SourceSession {
	async fn peer(&mut self, node: &str) -> Result<Option<Peer>> {
		Ok(Some(Peer {
			node_id: node.into(),
			endpoint: "https://receiver.invalid".into(),
			credential_env: "CREDENTIAL".into(),
			protocol_version: "current".into(),
			enabled: true,
		}))
	}
}
#[async_trait]
impl GrantScope for SourceSession {
	fn replace_subjects(&mut self, subjects: Vec<String>) {
		self.subjects = subjects;
	}
	fn append_subject(&mut self, subject: String) {
		self.subjects.push(subject);
	}
	async fn inherit_task_origin(&mut self, _: Uuid) -> Result<()> {
		self.repository.call("inherit_origin");
		Ok(())
	}
	async fn command_lock(&mut self, _: Uuid) -> Result<()> {
		self.repository.call("command_lock");
		Ok(())
	}
	async fn current_grant(&mut self, _: Uuid) -> Result<Grant> {
		self.repository.call("current_grant");
		self.current.clone().ok_or(Error::Forbidden)
	}
	async fn live(&mut self, _: Uuid) -> Result<bool> {
		self.repository.call("live");
		Ok(self.live)
	}
	async fn locked_task(&mut self, _: Uuid) -> Result<Task> {
		self.repository.call("locked_task");
		let mut task = self.task.clone();
		if self
			.races
			.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
			.is_ok()
		{
			task.revision += 1;
		}
		Ok(task)
	}
	async fn grant_reads_visible(&mut self, _: Uuid) -> Result<bool> {
		self.repository.call("grant_reads_visible");
		Ok(!self.hidden_reads)
	}
	async fn semantic_binding(
		&mut self,
		_: &Task,
		_: &str,
		_: &Inspection,
		_: &Request,
	) -> Result<Binding> {
		self.repository.call("semantic_binding");
		Ok(Binding::Disabled {})
	}
	async fn insert_grant(&mut self, _: &PrepareInput, _: &Task, _: &Value) -> Result<u64> {
		panic!("rejected preparation must not persist")
	}
	async fn persist_semantic(&mut self, _: Uuid, _: &Value) -> Result<()> {
		panic!("rejected preparation must not persist semantic binding")
	}
	async fn revocation_grant(&mut self, _: Uuid, _: Uuid) -> Result<Option<Grant>> {
		self.repository.call("revocation_grant");
		Ok(self.current.clone())
	}
	async fn revoke_locked(&mut self, _: Uuid) -> Result<()> {
		self.repository.call("revoke_locked");
		Ok(())
	}
	async fn event(&mut self, _: Uuid, kind: &str, _: Value) -> Result<()> {
		self.repository.call(kind);
		Ok(())
	}
}
fn input() -> PrepareInput {
	PrepareInput {
		id: Uuid::from_u128(4),
		node_id: "aidash://receiver".into(),
		agent: EntityRef {
			id: "agent".into(),
			version: "1".into(),
		},
		ttl_seconds: 3600,
		semantic: Request::Disabled {},
	}
}
#[rstest]
#[case("nil")]
#[case("local")]
#[case("short")]
#[case("long")]
#[tokio::test]
async fn invalid_preparation_never_opens_authority_or_persists(#[case] invalid: &str) {
	let repository = SourceRepository::new();
	let mut input = input();
	match invalid {
		"nil" => input.id = Uuid::nil(),
		"local" => input.node_id = "aidash://home".into(),
		"short" => input.ttl_seconds = 0,
		"long" => input.ttl_seconds = 3601,
		_ => panic!("unknown invalid input"),
	}
	assert!(
		matches!(use_case::prepare(&repository,task().id,input).await,Err(Error::Invalid(ref text)) if text=="invalid remote grant destination or lifetime")
	);
	assert!(repository.repository.calls().is_empty());
}
#[rstest]
#[tokio::test]
async fn preparation_inherits_original_authority_before_checking_assignment() {
	let repository = SourceRepository::new();
	assert!(
		matches!(use_case::prepare(&repository,task().id,input()).await,Err(Error::Conflict(ref text)) if text=="task is already assigned")
	);
	assert_eq!(
		repository.repository.calls(),
		vec![
			"source_begin",
			"inherit_origin",
			"task.read",
			"rollback_with_denial_audit"
		]
	);
}
#[rstest]
#[tokio::test]
async fn full_delegation_chain_is_rejected_before_receiver_inspection() {
	let mut repository = SourceRepository::new();
	repository.task.status = TaskStatus::Open;
	repository.subjects = (0..32).map(|n| format!("subject-{n}")).collect();
	assert!(
		matches!(use_case::prepare(&repository,task().id,input()).await,Err(Error::Invalid(ref text)) if text=="execution delegation depth exceeds 32")
	);
	assert!(!repository.repository.calls().contains(&"source_rpc".into()));
}
#[rstest]
#[tokio::test]
async fn non_agent_executor_is_rejected_before_disclosure() {
	let mut repository = SourceRepository::new();
	repository.task.status = TaskStatus::Open;
	repository.bundle.subjects.values_mut().next().unwrap().kind = SubjectKind::User;
	assert!(matches!(
		use_case::prepare(&repository, task().id, input()).await,
		Err(Error::Forbidden)
	));
	assert!(!repository.repository.calls().contains(&"source_rpc".into()));
}
#[rstest]
#[tokio::test]
async fn already_revoked_grant_has_no_second_revocation_event() {
	let mut repository = SourceRepository::new();
	repository.current.as_mut().unwrap().revoked = true;
	let prepared = use_case::revoke(&repository, task().id, Uuid::from_u128(4))
		.await
		.unwrap();
	assert!(prepared.revoked);
	assert!(
		!repository
			.repository
			.calls()
			.contains(&"revoke_locked".into())
	);
	assert!(
		!repository
			.repository
			.calls()
			.contains(&"task.remote_grant_revoked".into())
	);
	assert_eq!(repository.repository.calls().last().unwrap(), "commit");
}
#[rstest]
#[tokio::test]
async fn first_revocation_writes_under_its_lock_before_event_and_commit() {
	let repository = SourceRepository::new();
	assert!(
		use_case::revoke(&repository, task().id, Uuid::from_u128(4))
			.await
			.unwrap()
			.revoked
	);
	let calls = repository.repository.calls();
	let write = calls.iter().position(|c| c == "revoke_locked").unwrap();
	assert_eq!(
		&calls[write..],
		&["revoke_locked", "task.remote_grant_revoked", "commit"]
	);
}
#[rstest]
#[tokio::test]
async fn revoked_live_authority_is_not_retried_as_a_revision_race() {
	let mut repository = SourceRepository::new();
	repository.live = false;
	assert!(matches!(
		use_case::description(&repository, "aidash://receiver", Uuid::from_u128(4)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		repository
			.repository
			.calls()
			.iter()
			.filter(|c| c.as_str() == "begin_saved")
			.count(),
		1
	);
	assert!(!repository.repository.calls().contains(&"task.read".into()));
	assert_eq!(repository.repository.state.lock().unwrap().scopes, 0);
}
#[rstest]
#[tokio::test(start_paused = true)]
async fn read_only_revision_race_is_retried_at_most_three_times() {
	let repository = SourceRepository::new();
	repository.races.store(3, Ordering::SeqCst);
	let start = tokio::time::Instant::now();
	assert!(matches!(
		use_case::description(&repository, "aidash://receiver", Uuid::from_u128(4)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		repository
			.repository
			.calls()
			.iter()
			.filter(|c| c.as_str() == "begin_saved")
			.count(),
		3
	);
	assert_eq!(start.elapsed(), std::time::Duration::from_millis(20));
	assert_eq!(repository.repository.state.lock().unwrap().scopes, 0);
}
#[rstest]
#[tokio::test]
async fn command_lock_precedes_grant_and_task_row_locks_without_automatic_retry() {
	let repository = SourceRepository::new();
	repository.races.store(1, Ordering::SeqCst);
	let mut race = false;
	assert!(matches!(
		use_case::description_mode(
			&repository,
			"aidash://receiver",
			Uuid::from_u128(4),
			true,
			&mut race
		)
		.await,
		Err(Error::Forbidden)
	));
	assert!(race);
	let calls = repository.repository.calls();
	let journal = calls.iter().position(|c| c == "command_lock").unwrap();
	let grant = calls.iter().position(|c| c == "current_grant").unwrap();
	let task = calls.iter().position(|c| c == "locked_task").unwrap();
	assert!(journal < grant && grant < task);
	assert_eq!(
		calls.iter().filter(|c| c.as_str() == "begin_saved").count(),
		1
	);
}
#[rstest]
#[tokio::test]
async fn unavailable_saved_credentials_are_forbidden_without_retries() {
	let mut repository = SourceRepository::new();
	repository.saved_unauthorized = true;
	assert!(matches!(
		use_case::description(&repository, "aidash://receiver", Uuid::from_u128(4)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		repository.repository.calls(),
		vec!["source_grant", "begin_saved"]
	);
}
#[rstest]
#[tokio::test]
async fn hidden_source_reads_are_rejected_before_receiver_callback() {
	let mut repository = SourceRepository::new();
	repository.hidden_reads = true;
	assert!(matches!(
		use_case::description(&repository, "aidash://receiver", Uuid::from_u128(4)).await,
		Err(Error::Forbidden)
	));
	assert!(
		repository
			.repository
			.calls()
			.contains(&"grant_reads_visible".into())
	);
	assert!(!repository.repository.calls().contains(&"source_rpc".into()));
}
