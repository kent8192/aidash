use super::*;
use aidash_domain::{
	federation::execution::{
		Description, Inspection, Prepared,
		admission::{RemoteExecutionControlState, RemoteExecutionPhase},
		home::HomeBinding,
	},
	identity::execution::ExecutionPrincipal,
	policy::Resource,
	semantic::remote::status::Status as SemanticStatus,
};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use rstest::rstest;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
#[derive(Default)]
struct State {
	calls: Vec<String>,
	scopes: usize,
	rpcs: usize,
	max_rpcs: usize,
	denied: Option<&'static str>,
	replies: Vec<Value>,
	rpc_fails: bool,
	binding_changed: bool,
	grants: usize,
	semantic: bool,
	delay_status: bool,
	delay_recheck: bool,
	created: Option<(NewTask, String, String)>,
	cancelled: Option<(i64, String, Vec<String>)>,
	revision: Option<i64>,
	output_hidden: bool,
	human_requests: Vec<aidash_domain::HumanRequest>,
}
#[derive(Clone, Default)]
struct Repository {
	state: Arc<Mutex<State>>,
	operator: bool,
}
struct Scope {
	repository: Repository,
}
impl Drop for Scope {
	fn drop(&mut self) {
		self.repository.state.lock().unwrap().scopes -= 1;
	}
}
struct RpcGuard(Repository);
impl Drop for RpcGuard {
	fn drop(&mut self) {
		self.0.state.lock().unwrap().rpcs -= 1;
	}
}
fn principal() -> ExecutionPrincipal {
	ExecutionPrincipal {
		tenant: "tenant".into(),
		subject: "requester".into(),
		credential_id: Uuid::from_u128(1),
	}
}
fn task() -> Task {
	Task {
		id: Uuid::from_u128(2),
		workspace_id: Uuid::from_u128(3),
		title: "old intent".into(),
		description: "invalid producer context".into(),
		status: TaskStatus::Running,
		requirements: json!({}),
		owner: Some("agent".into()),
		created_by: "requester".into(),
		dependencies: vec![Uuid::from_u128(90)],
		parent_id: Some(Uuid::from_u128(91)),
		revision: 7,
		created_at: Utc.timestamp_opt(1, 0).unwrap(),
	}
}
fn grant(required: bool) -> Grant {
	let binding: Binding = if required {
		serde_json::from_value(json!({"mode":"required_home","home_lineage":[],"execution_lineage":[],"version":1,"index_revision":1,"index_digest":"index","embedding":{"node_id":"aidash://home","entry":{"id":"embedding","version":"1"},"digest":"digest","configuration_digest":"config"}})).unwrap()
	} else {
		Binding::Disabled {}
	};
	Grant {
		id: Uuid::from_u128(4),
		task_id: task().id,
		task_revision: 7,
		workspace_id: task().workspace_id,
		node_id: "aidash://receiver".into(),
		tenant: "tenant".into(),
		credential_id: principal().credential_id,
		root_subject: "requester".into(),
		subject_chain: vec!["requester".into()],
		inspection: json!({"node_id":"aidash://receiver","authority_digest":"authority","agent":crate::test_support::agent("agent"),"definitions":[],"binding_snapshot":crate::test_support::snapshot("aidash://receiver","agent")}),
		expires_at: Utc.timestamp_opt(3600, 0).unwrap(),
		revoked: false,
		semantic: serde_json::to_value(binding).unwrap(),
	}
}
fn description() -> Description {
	Description {
		grant_id: Uuid::from_u128(4),
		source_node: "aidash://home".into(),
		target_node: "aidash://receiver".into(),
		source_tenant: "tenant".into(),
		source_subject: "requester".into(),
		task: task(),
		inspection: serde_json::from_value::<Inspection>(grant(false).inspection).unwrap(),
		expires_at: grant(false).expires_at,
		semantic: Binding::Disabled {},
	}
}
fn activation() -> Value {
	json!({"grant_id":Uuid::from_u128(4),"admission_id":Uuid::from_u128(5),"run_id":Uuid::from_u128(5),"phase":RemoteExecutionPhase::Ready,"control":RemoteExecutionControlState::Active,"error":null})
}
impl Repository {
	fn call(&self, name: &str) {
		self.state.lock().unwrap().calls.push(name.into());
	}
	fn calls(&self) -> Vec<String> {
		self.state.lock().unwrap().calls.clone()
	}
	fn scope(&self) -> Scope {
		self.state.lock().unwrap().scopes += 1;
		Scope {
			repository: self.clone(),
		}
	}
}
#[async_trait]
impl HomeRepository for Repository {
	type Scope = Scope;
	fn node_id(&self) -> &str {
		"aidash://home"
	}
	fn identity(&self) -> Option<ExecutionPrincipal> {
		(!self.operator).then(principal)
	}
	async fn begin(&self) -> Result<Scope> {
		self.call("begin");
		let slow = {
			let state = self.state.lock().unwrap();
			state.delay_recheck
				&& state
					.calls
					.iter()
					.any(|c| c == "rpc:/scoped/execution/status")
		};
		if slow {
			tokio::time::sleep(std::time::Duration::from_secs(5)).await;
		}
		Ok(self.scope())
	}
	async fn description(&self, _: &str, _: Uuid) -> Result<(Scope, Description)> {
		self.call("description");
		Ok((self.scope(), description()))
	}
	async fn request<T: DeserializeOwned + Send>(
		&self,
		_: &str,
		path: &str,
		input: &Value,
	) -> Result<T> {
		let delay = {
			let mut state = self.state.lock().unwrap();
			assert_eq!(
				state.scopes, 0,
				"source locks must be released before peer RPC"
			);
			state.rpcs += 1;
			state.max_rpcs = state.max_rpcs.max(state.rpcs);
			state.calls.push(format!("rpc:{path}"));
			state.delay_status
		};
		let _guard = RpcGuard(self.clone());
		if path.ends_with("/status") && delay {
			tokio::time::sleep(std::time::Duration::from_secs(3)).await;
		}
		let value = {
			let mut state = self.state.lock().unwrap();
			if state.rpc_fails {
				return Err(Error::External("receiver unavailable".into()));
			}
			if !state.replies.is_empty() {
				state.replies.remove(0)
			} else if path.ends_with("/admissions") {
				json!({"id":Uuid::from_u128(5),"source_node":"aidash://home","grant_id":Uuid::from_u128(4),"task_id":task().id,"expires_at":grant(false).expires_at})
			} else if path.ends_with("/messages") {
				json!({"id":input["message"]["id"],"run_id":Uuid::from_u128(5),"accepted":true})
			} else if path.ends_with("/status") {
				Value::Null
			} else {
				activation()
			}
		};
		serde_json::from_value(value).map_err(Into::into)
	}
	async fn prepare(&self, _: Uuid, _: PrepareInput) -> Result<Prepared> {
		self.call("prepare");
		Ok(grant(false).prepared()?)
	}
	async fn semantic_status(
		&self,
		_: Uuid,
		binding: &Binding,
		reason: Option<Failure>,
	) -> Result<SemanticStatus> {
		Ok(aidash_domain::semantic::remote::status::project(
			binding, reason, None,
		)?)
	}
}
#[async_trait]
impl HomeScope for Scope {
	async fn human_requests(
		&mut self,
		_: Uuid,
		_: Uuid,
	) -> Result<Vec<aidash_domain::HumanRequest>> {
		self.repository.call("human_requests");
		Ok(self.repository.state.lock().unwrap().human_requests.clone())
	}
	async fn answer_human(
		&mut self,
		_: Uuid,
		_: Uuid,
		_: Uuid,
		_: Value,
	) -> Result<aidash_domain::HumanRequest> {
		Err(Error::Forbidden)
	}

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
		Ok(task())
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
#[rstest]
#[tokio::test]
async fn denied_message_authority_prevents_peer_call() {
	let repository = Repository::default();
	repository.state.lock().unwrap().denied = Some("run.message");
	let result = message(
		&repository,
		task().id,
		Uuid::from_u128(4),
		Message {
			id: Uuid::from_u128(6),
			content: "text".into(),
		},
	)
	.await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert!(!repository.calls().iter().any(|c| c.starts_with("rpc:")));
	assert_eq!(
		repository.calls().last().unwrap(),
		"rollback_with_denial_audit"
	);
	assert_eq!(repository.state.lock().unwrap().scopes, 0);
}
#[rstest]
#[case("id",json!(Uuid::from_u128(99)))]
#[case("run_id",json!(Uuid::from_u128(99)))]
#[case("accepted",json!(false))]
#[tokio::test]
async fn message_requires_matching_positive_receipt(#[case] field: &str, #[case] value: Value) {
	let repository = Repository::default();
	let mut receipt = json!({"id":Uuid::from_u128(6),"run_id":Uuid::from_u128(5),"accepted":true});
	receipt[field] = value;
	repository.state.lock().unwrap().replies.push(receipt);
	let result = message(
		&repository,
		task().id,
		Uuid::from_u128(4),
		Message {
			id: Uuid::from_u128(6),
			content: "text".into(),
		},
	)
	.await;
	assert!(matches!(result, Err(Error::Forbidden)));
}
#[rstest]
#[tokio::test]
async fn failed_receiver_cancel_keeps_home_ledger_open() {
	let repository = Repository::default();
	repository.state.lock().unwrap().rpc_fails = true;
	let result = control(
		&repository,
		task().id,
		Uuid::from_u128(4),
		RemoteExecutionControl::Cancel,
	)
	.await;
	assert!(matches!(result, Err(Error::External(_))));
	assert!(!repository.calls().contains(&"revoke".into()));
	assert!(repository.state.lock().unwrap().cancelled.is_none());
}
#[rstest]
#[tokio::test]
async fn cancellation_fences_delivered_messages_and_task_revision_after_receiver_stop() {
	let repository = Repository::default();
	control(
		&repository,
		task().id,
		Uuid::from_u128(4),
		RemoteExecutionControl::Cancel,
	)
	.await
	.unwrap();
	let calls = repository.calls();
	let rpc = calls.iter().position(|c| c.ends_with("/control")).unwrap();
	let revoke = calls.iter().position(|c| c == "revoke").unwrap();
	let cancel = calls.iter().position(|c| c == "cancel_task").unwrap();
	assert!(rpc < revoke && revoke < cancel);
	let state = repository.state.lock().unwrap();
	assert_eq!(
		state.cancelled,
		Some((
			7,
			qualified_agent("aidash://receiver", "agent", "1.0.0"),
			vec!["delivered-key".into()]
		))
	);
	assert_eq!(state.revision, Some(8));
	assert!(!calls.contains(&"task.read".into()));
}
#[rstest]
#[tokio::test]
async fn resume_rechecks_semantic_authority_before_receiver_control() {
	let repository = Repository::default();
	repository.state.lock().unwrap().semantic = true;
	control(
		&repository,
		task().id,
		Uuid::from_u128(4),
		RemoteExecutionControl::Resume,
	)
	.await
	.unwrap();
	let calls = repository.calls();
	let reset = calls.iter().position(|c| c == "resume_semantic").unwrap();
	let rpc = calls.iter().position(|c| c.ends_with("/control")).unwrap();
	assert!(reset < rpc);
	assert_eq!(calls[reset + 1], "commit");
	assert!(calls.contains(&"task.read".into()));
}
#[rstest]
#[tokio::test]
async fn changed_initial_task_binding_blocks_receiver_activation() {
	let repository = Repository::default();
	repository.state.lock().unwrap().binding_changed = true;
	let result = activate(&repository, task().id, Uuid::from_u128(4)).await;
	assert!(
		matches!(result,Err(Error::Conflict(ref text)) if text=="remote execution identity changed")
	);
	assert_eq!(
		repository
			.calls()
			.iter()
			.filter(|c| c.starts_with("rpc:"))
			.count(),
		1
	);
	assert_eq!(
		repository.calls().last().unwrap(),
		"rollback_with_denial_audit"
	);
}
#[rstest]
#[case("grant_id")]
#[case("source_node")]
#[case("task_id")]
#[tokio::test]
async fn unrelated_admission_is_rejected_before_creating_home_binding(#[case] field: &str) {
	let repository = Repository::default();
	let mut value = json!({"id":Uuid::from_u128(5),"source_node":"aidash://home","grant_id":Uuid::from_u128(4),"task_id":task().id,"expires_at":grant(false).expires_at});
	value[field] = if field == "source_node" {
		json!("aidash://foreign")
	} else {
		json!(Uuid::from_u128(99))
	};
	repository.state.lock().unwrap().replies.push(value);
	assert!(matches!(
		activate(&repository, task().id, Uuid::from_u128(4)).await,
		Err(Error::Forbidden)
	));
	assert!(!repository.calls().contains(&"description".into()));
}
#[rstest]
#[tokio::test]
async fn follow_up_creates_independent_intent_with_durable_audit_key() {
	let repository = Repository::default();
	follow_up(
		&repository,
		task().id,
		Uuid::from_u128(4),
		FollowUpInput {
			id: Uuid::from_u128(6),
			title: "fresh title".into(),
			description: "fresh intent".into(),
			requirements: Default::default(),
		},
	)
	.await
	.unwrap();
	let state = repository.state.lock().unwrap();
	let (input, subject, key) = state.created.as_ref().unwrap();
	assert_eq!(input.title, "fresh title");
	assert_eq!(input.description, "fresh intent");
	assert_eq!(input.dependencies, Vec::<Uuid>::new());
	assert_eq!(input.parent_id, None);
	assert_eq!(subject, "requester");
	assert_eq!(
		key,
		&format!(
			"remote-follow-up:{}:{}",
			Uuid::from_u128(4),
			Uuid::from_u128(6)
		)
	);
}
#[rstest]
#[tokio::test(start_paused = true)]
async fn status_fanout_is_bounded_and_preserves_grant_order() {
	let repository = Repository::default();
	{
		let mut state = repository.state.lock().unwrap();
		state.grants = 7;
		state.delay_status = true;
	}
	let statuses = list(&repository, task().id).await.unwrap();
	assert_eq!(
		statuses.iter().map(|s| s.grant.id).collect::<Vec<_>>(),
		(4..11).map(Uuid::from_u128).collect::<Vec<_>>()
	);
	assert!(statuses.iter().all(|s| s.unavailable));
	assert_eq!(repository.state.lock().unwrap().max_rpcs, 4);
	assert_eq!(repository.state.lock().unwrap().rpcs, 0);
}
#[rstest]
#[tokio::test(start_paused = true)]
async fn slow_peer_status_preserves_home_owned_human_continuations() {
	let repository = Repository::default();
	let request = aidash_domain::HumanRequest {
		id: Uuid::from_u128(8),
		run_id: Uuid::from_u128(5),
		workspace_id: task().workspace_id,
		kind: "APPROVAL_REQUIRED".into(),
		prompt: "Approve the saved effect".into(),
		response: None,
		answered_by: None,
		created_at: Utc.timestamp_opt(1, 0).unwrap(),
	};
	{
		let mut state = repository.state.lock().unwrap();
		state.delay_status = true;
		state.human_requests = vec![request.clone()];
	}
	let start = tokio::time::Instant::now();
	let statuses = list(&repository, task().id).await.unwrap();
	assert_eq!(start.elapsed(), std::time::Duration::from_secs(2));
	assert!(statuses[0].unavailable);
	assert_eq!(statuses[0].human_requests.len(), 1);
	assert_eq!(statuses[0].human_requests[0].id, request.id);
	assert!(statuses[0].human_requests[0].response.is_none());
	let calls = repository.calls();
	assert!(
		calls
			.iter()
			.position(|call| call == "human_requests")
			.unwrap() < calls
			.iter()
			.position(|call| call == "rpc:/scoped/execution/status")
			.unwrap()
	);
	assert_eq!(repository.state.lock().unwrap().scopes, 0);
}
#[rstest]
#[tokio::test(start_paused = true)]
async fn authority_recheck_uses_the_same_status_deadline() {
	let repository = Repository::default();
	{
		let mut state = repository.state.lock().unwrap();
		state.semantic = true;
		state.delay_recheck = true;
	}
	let start = tokio::time::Instant::now();
	let statuses = list(&repository, task().id).await.unwrap();
	assert_eq!(start.elapsed(), std::time::Duration::from_secs(2));
	assert!(statuses[0].unavailable);
	assert_eq!(statuses[0].semantic.reason, Some(Failure::Unavailable));
	assert_eq!(repository.state.lock().unwrap().scopes, 0);
}
#[rstest]
#[tokio::test]
async fn hidden_grant_output_prevents_provenance_reads() {
	let repository = Repository::default();
	repository.state.lock().unwrap().output_hidden = true;
	assert!(matches!(
		provenance(&repository, task().id, Uuid::from_u128(4)).await,
		Err(Error::Forbidden)
	));
	assert!(!repository.calls().contains(&"receipt".into()));
}
#[rstest]
#[tokio::test]
async fn operator_cannot_create_subject_remote_work() {
	let repository = Repository {
		operator: true,
		..Default::default()
	};
	assert!(matches!(
		activate(&repository, task().id, Uuid::from_u128(4)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.calls(), Vec::<String>::new());
}

mod source_grants;
