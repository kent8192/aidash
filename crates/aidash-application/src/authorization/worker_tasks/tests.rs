use super::*;
use crate::ports::authorization::worker_tasks::WorkerTaskScope;
use aidash_domain::{
	RunControl, RunPhase, TaskStatus,
	generation::requests::Request,
	identity::execution::{ExecutionGrant, ExecutionPrincipal},
	policy::Resource,
};
use async_trait::async_trait;
use chrono::Utc;
use rstest::{fixture, rstest};
use serde_json::json;
fn id(value: u128) -> Uuid {
	Uuid::from_u128(value)
}
#[fixture]
fn run() -> RunMetadata {
	RunMetadata {
		id: id(1),
		task_id: id(2),
		workspace_id: id(3),
		home_node: "aidash://local".into(),
		agent_id: "producer".into(),
		agent_version: "1".into(),
		phase: RunPhase::Ready,
		control: RunControl::Active,
		step: 0,
		revision: 5,
		observed_input_seq: 0,
		ledger_worker_ready: false,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: Utc::now(),
	}
}
fn input() -> NewTask {
	NewTask {
		title: "child".into(),
		description: "delegated work".into(),
		requirements: json!({"preserved":true}),
		dependencies: vec![id(2)],
		parent_id: Some(id(2)),
	}
}
fn agent() -> EntityRef {
	EntityRef {
		id: "coordinator".into(),
		version: "1".into(),
	}
}
fn delegated() -> Delegation {
	Delegation {
		task_id: id(4),
		node_id: "aidash://local".into(),
		agent_id: "coordinator".into(),
		agent_version: "1".into(),
		delivered: false,
	}
}
fn generated() -> Request {
	Request {
		id: id(9),
		tenant: "tenant".into(),
		policy_id: "policy".into(),
		policy_revision: 3,
		task_id: id(4),
		home_node: "aidash://local".into(),
		foreign_intent: None,
		prepared: false,
		grant_id: None,
		admission_id: None,
		workspace_id: id(3),
		credential_id: id(8),
		root_subject: "root".into(),
		subject_chain: vec!["root".into(), "producer".into()],
		agent_id: "generated".into(),
		agent_version: "1".into(),
		definition: json!({}),
		status: "PENDING_APPROVAL".into(),
		reason: "reason".into(),
		depth: 1,
		token_limit: 100,
		quota_released: false,
		expires_at: Utc::now(),
		created_at: Utc::now(),
	}
}
struct Scope {
	node: String,
	principal: ExecutionPrincipal,
	subjects: Vec<String>,
	workspace: Option<Uuid>,
	task: Task,
	origin: CreatedTaskOrigin,
	grant: Option<ExecutionGrant>,
	assignment: Assignment,
	calls: Vec<&'static str>,
	outputs: Vec<(Uuid, Uuid, String, Uuid)>,
	creations: Vec<(Uuid, serde_json::Value, String, String)>,
	origin_writes: Vec<(Uuid, Uuid)>,
	decisions: Vec<(Resource, String)>,
	denied: Option<&'static str>,
	fail: Option<&'static str>,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		node: "aidash://local".into(),
		principal: ExecutionPrincipal {
			tenant: "tenant".into(),
			subject: "root".into(),
			credential_id: id(8),
		},
		subjects: vec!["root".into(), "producer".into()],
		workspace: Some(id(3)),
		task: Task {
			id: id(4),
			workspace_id: id(3),
			title: "saved child".into(),
			description: "saved description".into(),
			status: TaskStatus::Open,
			requirements: json!({}),
			owner: None,
			created_by: "producer".into(),
			dependencies: vec![],
			parent_id: Some(id(2)),
			revision: 1,
			created_at: Utc::now(),
		},
		origin: CreatedTaskOrigin {
			source_run_id: id(1),
			authority: TaskOrigin {
				tenant: "tenant".into(),
				root_subject: "root".into(),
				subject_chain: vec!["root".into(), "producer".into()],
			},
		},
		grant: None,
		assignment: Assignment::Existing {
			delegation: delegated(),
		},
		calls: vec![],
		outputs: vec![],
		creations: vec![],
		origin_writes: vec![],
		decisions: vec![],
		denied: None,
		fail: None,
	}
}
impl Scope {
	fn touch(&mut self, call: &'static str) -> Result<()> {
		self.calls.push(call);
		if self.fail == Some(call) {
			return Err(Error::Port(Box::new(std::io::Error::other(call))));
		}
		if self.denied == Some(call) {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	fn resource(&self, kind: &str, value: Uuid, attributes: serde_json::Value) -> Resource {
		Resource {
			tenant: self.principal.tenant.clone(),
			kind: kind.into(),
			id: value.to_string(),
			attributes,
		}
	}
	fn grant(&self) -> ExecutionGrant {
		ExecutionGrant {
			run_id: id(7),
			task_id: id(4),
			workspace_id: id(3),
			tenant: "tenant".into(),
			credential_id: id(8),
			root_subject: "root".into(),
			subject_chain: vec![
				"root".into(),
				"producer".into(),
				qualified_agent(&self.node, "coordinator", "1"),
			],
		}
	}
}
#[async_trait]
impl WorkerTaskScope for Scope {
	fn node_id(&self) -> &str {
		&self.node
	}
	fn identity(&self) -> ExecutionPrincipal {
		self.principal.clone()
	}
	fn subjects(&self) -> &[String] {
		&self.subjects
	}
	async fn task_workspace(&mut self, task: Uuid) -> Result<Option<Uuid>> {
		assert_eq!(task, self.task.id);
		self.touch("task_workspace")?;
		Ok(self.workspace)
	}
	async fn assign(&mut self, task: Uuid, policy: &str, reason: &str) -> Result<Assignment> {
		assert_eq!(task, self.task.id);
		assert_eq!(policy, "policy");
		assert_eq!(reason, "reason");
		self.touch("assign")?;
		Ok(self.assignment.clone())
	}
	async fn record_output(&mut self, source: &RunMetadata, kind: &str, value: Uuid) -> Result<()> {
		self.touch(match kind {
			"task" => "output.task",
			"generation" => "output.generation",
			_ => panic!("unknown output"),
		})?;
		self.outputs
			.push((source.id, source.workspace_id, kind.into(), value));
		Ok(())
	}
	async fn workspace(&mut self, value: Uuid) -> Result<Resource> {
		assert_eq!(value, id(3));
		self.touch("workspace")?;
		Ok(self.resource("workspace", value, json!({"owner":"saved-owner"})))
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		assert_eq!(resource.tenant, "tenant");
		self.decisions.push((resource.clone(), action.into()));
		self.touch(match action {
			"task.create" => "task.create",
			"task.read" => "task.read",
			"task.delegate" => "task.delegate",
			_ => panic!("unexpected policy action"),
		})
	}
	async fn related_tasks(&mut self, workspace: Uuid, input: &NewTask) -> Result<()> {
		assert_eq!(workspace, id(3));
		assert_eq!(input.dependencies, vec![id(2)]);
		assert_eq!(input.parent_id, Some(id(2)));
		self.touch("related")
	}
	async fn create_task(
		&mut self,
		workspace: Uuid,
		input: &NewTask,
		creator: &str,
		key: &str,
	) -> Result<Task> {
		self.touch("create")?;
		self.creations.push((
			workspace,
			serde_json::to_value(input).unwrap(),
			creator.into(),
			key.into(),
		));
		Ok(self.task.clone())
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.touch("task_resource")?;
		Ok(self.resource(
			"task",
			task.id,
			json!({"created_by":task.created_by,"workspace_id":task.workspace_id}),
		))
	}
	async fn insert_origin(&mut self, task: Uuid, source: Uuid) -> Result<()> {
		self.touch("insert_origin")?;
		self.origin_writes.push((task, source));
		Ok(())
	}
	async fn created_origin(&mut self, task: Uuid) -> Result<CreatedTaskOrigin> {
		assert_eq!(task, self.task.id);
		self.touch("origin")?;
		Ok(self.origin.clone())
	}
	async fn task_read(&mut self, task: Uuid) -> Result<Task> {
		assert_eq!(task, self.task.id);
		self.touch("task_read")?;
		Ok(self.task.clone())
	}
	async fn execution_grant(&mut self, task: Uuid) -> Result<Option<ExecutionGrant>> {
		assert_eq!(task, self.task.id);
		self.touch("grant")?;
		Ok(self.grant.clone())
	}
	async fn delegate(&mut self, task: Uuid, reference: &EntityRef) -> Result<Delegation> {
		assert_eq!(task, self.task.id);
		assert_eq!(*reference, agent());
		self.touch("delegate")?;
		Ok(delegated())
	}
}
#[rstest]
#[case::existing(false)]
#[case::generated(true)]
#[tokio::test]
async fn assignment_records_every_output_before_returning_it(
	mut scope: Scope,
	run: RunMetadata,
	#[case] generate: bool,
) {
	if generate {
		scope.assignment = Assignment::Generated {
			generation: Box::new(generated()),
		};
	}
	let expected = serde_json::to_value(&scope.assignment).unwrap();
	let output = assign(&mut scope, &run, id(4), "policy", "reason")
		.await
		.unwrap();
	assert_eq!(serde_json::to_value(output).unwrap(), expected);
	let mut expected = vec![(id(1), id(3), "task".into(), id(4))];
	if generate {
		expected.push((id(1), id(3), "generation".into(), id(9)));
	}
	assert_eq!(scope.outputs, expected);
	assert_eq!(
		scope.calls.last(),
		Some(&if generate {
			"output.generation"
		} else {
			"output.task"
		})
	);
}
#[rstest]
#[case::missing_assign(None, false)]
#[case::foreign_assign(Some(id(99)), false)]
#[case::missing_delegate(None, true)]
#[case::foreign_delegate(Some(id(99)), true)]
#[tokio::test]
async fn worker_effects_cannot_target_tasks_outside_the_source_workspace(
	mut scope: Scope,
	run: RunMetadata,
	#[case] workspace: Option<Uuid>,
	#[case] delegation: bool,
) {
	scope.workspace = workspace;
	let denied = if delegation {
		matches!(
			delegate(&mut scope, &run, id(4), "aidash://local", &agent()).await,
			Err(Error::Forbidden)
		)
	} else {
		matches!(
			assign(&mut scope, &run, id(4), "policy", "reason").await,
			Err(Error::Forbidden)
		)
	};
	assert!(denied);
	assert_eq!(scope.calls, vec!["task_workspace"]);
	assert!(scope.outputs.is_empty());
}
#[rstest]
#[case::workspace("task_workspace")]
#[case::assignment("assign")]
#[case::task_journal("output.task")]
#[case::generation_journal("output.generation")]
#[tokio::test]
async fn assignment_faults_do_not_disclose_a_partial_assignment(
	mut scope: Scope,
	run: RunMetadata,
	#[case] fail: &'static str,
) {
	scope.assignment = Assignment::Generated {
		generation: Box::new(generated()),
	};
	scope.fail = Some(fail);
	let Error::Port(error) = assign(&mut scope, &run, id(4), "policy", "reason")
		.await
		.err()
		.unwrap()
	else {
		panic!("expected adapter fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		fail
	);
	assert_eq!(scope.calls.last(), Some(&fail));
}
#[rstest]
#[tokio::test]
async fn a_child_task_retains_the_final_delegated_creator_key_and_producer(
	mut scope: Scope,
	run: RunMetadata,
) {
	let output = create_task(&mut scope, &run, "run:1:call:original", &input())
		.await
		.unwrap();
	assert_eq!(output.id, id(4));
	assert_eq!(
		scope.creations,
		vec![(
			id(3),
			serde_json::to_value(input()).unwrap(),
			"producer".into(),
			"run:1:call:original".into()
		)]
	);
	assert_eq!(scope.outputs, vec![(id(1), id(3), "task".into(), id(4))]);
	assert_eq!(scope.origin_writes, vec![(id(4), id(1))]);
	assert_eq!(scope.decisions[0].0.attributes["owner"], "saved-owner");
	assert_eq!(scope.decisions[1].0.attributes["created_by"], "producer");
	assert_eq!(
		scope.calls,
		vec![
			"workspace",
			"task.create",
			"related",
			"create",
			"task_resource",
			"task.read",
			"output.task",
			"insert_origin",
			"origin"
		]
	);
}
#[rstest]
#[tokio::test]
async fn an_empty_delegated_chain_is_rejected_after_related_task_authority(
	mut scope: Scope,
	run: RunMetadata,
) {
	scope.subjects.clear();
	assert!(matches!(
		create_task(&mut scope, &run, "key", &input()).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["workspace", "task.create", "related"]);
	assert!(scope.creations.is_empty());
	assert!(scope.origin_writes.is_empty());
}
#[rstest]
#[case::producer("producer")]
#[case::tenant("tenant")]
#[case::root("root")]
#[case::chain("chain")]
#[tokio::test]
async fn retries_cannot_rebind_an_existing_child_task_origin(
	mut scope: Scope,
	run: RunMetadata,
	#[case] different: &str,
) {
	match different {
		"producer" => scope.origin.source_run_id = id(99),
		"tenant" => scope.origin.authority.tenant = "other".into(),
		"root" => scope.origin.authority.root_subject = "other".into(),
		"chain" => scope.origin.authority.subject_chain.push("other".into()),
		_ => panic!("unknown fence"),
	};
	let Error::Conflict(message) = create_task(&mut scope, &run, "key", &input())
		.await
		.err()
		.unwrap()
	else {
		panic!("expected origin conflict")
	};
	assert_eq!(message, "task already has a different origin");
	assert_eq!(scope.calls.last(), Some(&"origin"));
	assert_eq!(scope.origin_writes, vec![(id(4), id(1))]);
}
#[rstest]
#[case::creation("task.create",vec!["workspace","task.create"])]
#[case::disclosure("task.read",vec!["workspace","task.create","related","create","task_resource","task.read"])]
#[tokio::test]
async fn child_task_creation_and_returned_record_are_both_authorized(
	mut scope: Scope,
	run: RunMetadata,
	#[case] denied: &'static str,
	#[case] calls: Vec<&'static str>,
) {
	scope.denied = Some(denied);
	assert!(matches!(
		create_task(&mut scope, &run, "key", &input()).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, calls);
	assert!(scope.outputs.is_empty());
	assert!(scope.origin_writes.is_empty());
}
#[rstest]
#[case::workspace("workspace")]
#[case::creation_policy("task.create")]
#[case::related("related")]
#[case::insert("create")]
#[case::resource("task_resource")]
#[case::disclosure_policy("task.read")]
#[case::output("output.task")]
#[case::origin_write("insert_origin")]
#[case::origin_read("origin")]
#[tokio::test]
async fn child_task_faults_keep_their_identity_and_stop_before_disclosure(
	mut scope: Scope,
	run: RunMetadata,
	#[case] fail: &'static str,
) {
	scope.fail = Some(fail);
	let Error::Port(error) = create_task(&mut scope, &run, "key", &input())
		.await
		.err()
		.unwrap()
	else {
		panic!("expected adapter fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		fail
	);
	assert_eq!(scope.calls.last(), Some(&fail));
}
#[rstest]
#[tokio::test]
async fn worker_delegation_remains_local_and_denies_foreign_nodes_before_task_io(
	mut scope: Scope,
	run: RunMetadata,
) {
	assert!(matches!(
		delegate(&mut scope, &run, id(4), "aidash://other", &agent()).await,
		Err(Error::Forbidden)
	));
	assert!(scope.calls.is_empty());
	assert!(scope.decisions.is_empty());
}
#[rstest]
#[tokio::test]
async fn a_matching_existing_grant_returns_the_original_delivered_reply_without_readmitting(
	mut scope: Scope,
	run: RunMetadata,
) {
	scope.grant = Some(scope.grant());
	let result = delegate(&mut scope, &run, id(4), "aidash://local", &agent())
		.await
		.unwrap();
	assert_eq!(result.task_id, id(4));
	assert_eq!(result.agent_id, "coordinator");
	assert_eq!(result.agent_version, "1");
	assert_eq!(result.node_id, "aidash://local");
	assert!(result.delivered);
	assert!(!scope.calls.contains(&"delegate"));
	assert_eq!(
		scope.calls,
		vec![
			"task_workspace",
			"task_read",
			"task_resource",
			"task.delegate",
			"grant"
		]
	);
}
#[rstest]
#[case::chain(true)]
#[case::credential(false)]
#[tokio::test]
async fn an_existing_grant_cannot_hide_changed_delegation_authority(
	mut scope: Scope,
	run: RunMetadata,
	#[case] chain: bool,
) {
	let mut grant = scope.grant();
	if chain {
		grant.subject_chain.push("other".into());
	} else {
		grant.credential_id = id(99);
	}
	scope.grant = Some(grant);
	let Error::Conflict(message) = delegate(&mut scope, &run, id(4), "aidash://local", &agent())
		.await
		.err()
		.unwrap()
	else {
		panic!("expected authority conflict")
	};
	assert_eq!(message, "task already has a different authority");
	assert_eq!(scope.calls.last(), Some(&"grant"));
	assert!(!scope.calls.contains(&"delegate"));
}
#[rstest]
#[tokio::test]
async fn fresh_delegation_reuses_local_admission_only_after_task_authority(
	mut scope: Scope,
	run: RunMetadata,
) {
	let result = delegate(&mut scope, &run, id(4), "aidash://local", &agent())
		.await
		.unwrap();
	assert!(!result.delivered);
	assert_eq!(
		scope.calls,
		vec![
			"task_workspace",
			"task_read",
			"task_resource",
			"task.delegate",
			"grant",
			"delegate"
		]
	);
	assert_eq!(scope.decisions[0].0.attributes["created_by"], "producer");
}
#[rstest]
#[tokio::test]
async fn delegation_denial_precedes_grant_presence_or_any_external_effect(
	mut scope: Scope,
	run: RunMetadata,
) {
	scope.grant = Some(scope.grant());
	scope.denied = Some("task.delegate");
	assert!(matches!(
		delegate(&mut scope, &run, id(4), "aidash://local", &agent()).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		scope.calls,
		vec![
			"task_workspace",
			"task_read",
			"task_resource",
			"task.delegate"
		]
	);
	assert!(!scope.calls.contains(&"grant"));
}
#[rstest]
#[case::workspace("task_workspace")]
#[case::task("task_read")]
#[case::resource("task_resource")]
#[case::policy("task.delegate")]
#[case::grant("grant")]
#[case::admission("delegate")]
#[tokio::test]
async fn delegation_faults_do_not_become_successful_retry_replies(
	mut scope: Scope,
	run: RunMetadata,
	#[case] fail: &'static str,
) {
	scope.fail = Some(fail);
	let Error::Port(error) = delegate(&mut scope, &run, id(4), "aidash://local", &agent())
		.await
		.err()
		.unwrap()
	else {
		panic!("expected adapter fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		fail
	);
	assert_eq!(scope.calls.last(), Some(&fail));
}
