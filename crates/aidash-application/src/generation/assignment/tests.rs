use super::*;
use crate::{
	generation::test_support,
	ports::generation::assignment::GenerationAssignmentSession,
	ports::{Credentials, registry::CoreToolCatalog},
};
use aidash_domain::{
	capabilities::CoreCapabilities,
	generation::requests::Request,
	policy::{PolicyBundle, Subject},
	provider::ToolSpec,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::{fixture, rstest};
use serde_json::Value;
use std::{
	collections::{BTreeMap, BTreeSet},
	sync::{Arc, Mutex},
	time::Duration,
};
struct NoSecrets;
impl Credentials for NoSecrets {
	fn resolve(&self, _: &str) -> Result<String> {
		panic!("generation admission must not execute a provider")
	}
}
struct CoreContracts;
impl CoreToolCatalog for CoreContracts {
	fn specifications(&self, _: &CoreCapabilities) -> BTreeMap<String, ToolSpec> {
		BTreeMap::new()
	}
}
#[fixture]
fn validation() -> DefinitionValidation {
	DefinitionValidation::new(Arc::new(NoSecrets), Arc::new(CoreContracts))
}
#[derive(Default)]
struct State {
	calls: Vec<String>,
	effects: Vec<String>,
	failure: Option<String>,
	pause: Option<String>,
	denied: BTreeSet<String>,
	active: usize,
	commits: usize,
	rollbacks: usize,
	notifications: usize,
}
#[derive(Clone)]
struct Scope {
	task: Task,
	policy: Policy,
	chain: Vec<String>,
	bundle: PolicyBundle,
	context: Value,
	inherited: bool,
	existing: Option<Request>,
	run: Option<(String, String, Vec<String>)>,
	catalog: Vec<Entry>,
	generated: BTreeSet<String>,
	depth: Option<i32>,
	active: i64,
	visible: bool,
	state: Arc<Mutex<State>>,
	effects: Vec<String>,
	owned: bool,
	finished: bool,
	created: Option<Request>,
	foreign_clock: DateTime<Utc>,
}
impl Scope {
	fn new() -> Self {
		let now = DateTime::<Utc>::from_timestamp(1000, 0).unwrap();
		Self {
			task: Task {
				id: Uuid::from_u128(2),
				workspace_id: Uuid::from_u128(3),
				title: "Task".into(),
				description: "Do work".into(),
				status: TaskStatus::Open,
				requirements: json!({}),
				owner: None,
				created_by: "alice".into(),
				dependencies: vec![],
				parent_id: None,
				revision: 0,
				created_at: now,
			},
			policy: Policy {
				tenant: "tenant".into(),
				id: "policy".into(),
				revision: 7,
				spec: serde_json::from_value(test_support::specification()).unwrap(),
				generated_count: 0,
				allocated_tokens: 0,
				allocated_compaction_calls: 0,
				allocated_embedding_calls: 0,
			},
			chain: vec!["alice".into()],
			bundle: serde_json::from_value(json!({"tenant":"tenant"})).unwrap(),
			context: json!({}),
			inherited: false,
			existing: None,
			run: None,
			catalog: vec![],
			generated: BTreeSet::new(),
			depth: None,
			active: 0,
			visible: true,
			state: Arc::new(Mutex::new(State::default())),
			effects: vec![],
			owned: false,
			finished: false,
			created: None,
			foreign_clock: now,
		}
	}
	async fn point(&self, name: impl Into<String>) -> Result<()> {
		let name = name.into();
		let (fail, pause) = {
			let mut state = self.state.lock().unwrap();
			state.calls.push(name.clone());
			(
				state.failure.as_ref() == Some(&name),
				state.pause.as_ref() == Some(&name),
			)
		};
		if fail {
			return Err(Error::External(format!("{name} failed")));
		}
		if pause {
			std::future::pending::<()>().await;
		}
		Ok(())
	}
	fn calls(&self) -> Vec<String> {
		self.state.lock().unwrap().calls.clone()
	}
	fn ordinary(&mut self, id: &str, enabled: bool, kind: SubjectKind) {
		let mut entry = self.policy.spec.template.clone();
		entry.id = id.into();
		self.catalog.push(entry);
		self.bundle.subjects.insert(
			qualified_agent("aidash://local", id, "1.0.0"),
			Subject {
				kind,
				enabled,
				groups: BTreeSet::new(),
				roles: BTreeSet::new(),
				attributes: json!({}),
				delegated_by: Some("alice".into()),
			},
		);
	}
}
impl Drop for Scope {
	fn drop(&mut self) {
		if self.owned && !self.finished {
			let mut state = self.state.lock().unwrap();
			state.active -= 1;
			state.rollbacks += 1;
			state.calls.push("drop".into());
		}
	}
}
struct Repository {
	scope: Scope,
}
impl Repository {
	fn new() -> Self {
		Self {
			scope: Scope::new(),
		}
	}
}
#[async_trait]
impl GenerationAssignments for Repository {
	async fn begin(&self) -> Result<Box<dyn GenerationAssignmentSession>> {
		let mut scope = self.scope.clone();
		scope.owned = true;
		let mut state = scope.state.lock().unwrap();
		state.active += 1;
		state.calls.push("begin".into());
		drop(state);
		Ok(Box::new(scope))
	}
	fn notify(&self) {
		let mut state = self.scope.state.lock().unwrap();
		assert_eq!(state.active, 0);
		state.notifications += 1;
		state.calls.push("notify".into());
	}
}
#[async_trait]
impl GenerationAssignmentSession for Scope {
	async fn finish(mut self: Box<Self>, result: Result<Assignment>) -> Result<Assignment> {
		self.point("finish").await?;
		let mut state = self.state.lock().unwrap();
		state.active -= 1;
		self.finished = true;
		if result.is_ok() && state.failure.as_deref() == Some("commit") {
			state.rollbacks += 1;
			return Err(Error::External("commit failed".into()));
		}
		if result.is_ok() {
			state.commits += 1;
			state.effects.append(&mut self.effects);
		} else {
			state.rollbacks += 1;
		}
		result
	}
}
#[async_trait]
impl GenerationCreationScope for Scope {
	fn tenant(&self) -> &str {
		"tenant"
	}
	fn subject(&self) -> &str {
		"alice"
	}
	fn subjects(&self) -> &[String] {
		&self.chain
	}
	fn bundle(&self) -> &PolicyBundle {
		&self.bundle
	}
	fn node_id(&self) -> &str {
		"aidash://local"
	}
	fn now(&self) -> DateTime<Utc> {
		self.foreign_clock
	}
	fn request_id(&self) -> Uuid {
		Uuid::from_u128(1)
	}
	async fn catalog_entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		self.point(format!("catalog:{}:{action}", reference.id))
			.await?;
		let mut entry = self.policy.spec.template.clone();
		entry.id = reference.id.clone();
		entry.version = reference.version.clone();
		Ok(entry)
	}
	async fn previous_depth(&mut self) -> Result<Option<i32>> {
		self.point("depth").await?;
		Ok(self.depth)
	}
	async fn active(&mut self, policy_id: &str) -> Result<i64> {
		assert_eq!(policy_id, "policy");
		self.point("active").await?;
		Ok(self.active)
	}
	async fn insert(&mut self, input: &Creation<'_>) -> Result<Request> {
		self.point("insert").await?;
		self.effects.push("insert".into());
		let mut job = test_support::request(input.id.as_u128());
		job.policy_revision = input.policy.revision;
		job.agent_id = input.definition.id.clone();
		job.agent_version = input.definition.version.clone();
		job.definition = json!(input.definition);
		job.status = input.status.into();
		job.reason = input.reason.into();
		job.depth = input.depth;
		job.token_limit = input.policy.spec.limits.tokens_per_agent;
		job.home_node = input.home_node.into();
		job.foreign_intent = input.foreign_intent.clone();
		job.subject_chain = self.chain.clone();
		job.expires_at =
			self.foreign_clock + chrono::Duration::seconds(input.lifetime_seconds as i64);
		self.created = Some(job.clone());
		Ok(job)
	}
	async fn allocate(
		&mut self,
		policy: &Policy,
		compaction_calls: i64,
		embedding_calls: i64,
	) -> Result<()> {
		self.point("allocate").await?;
		self.effects.push(format!(
			"allocate:{}:{compaction_calls}:{embedding_calls}",
			policy.spec.limits.tokens_per_agent
		));
		Ok(())
	}
	async fn budget(
		&mut self,
		id: Uuid,
		_policy: &Policy,
		compaction_calls: i64,
		embedding_calls: i64,
	) -> Result<()> {
		assert_eq!(id, Uuid::from_u128(1));
		self.point("budget").await?;
		self.effects
			.push(format!("budget:{compaction_calls}:{embedding_calls}"));
		Ok(())
	}
	async fn history(&mut self, id: Uuid, status: &str, reason: &str) -> Result<()> {
		assert_eq!(id, Uuid::from_u128(1));
		assert_eq!(reason, "work");
		self.point("history").await?;
		self.effects.push(format!("history:{status}"));
		Ok(())
	}
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()> {
		assert_eq!(workspace, Uuid::from_u128(3));
		assert_eq!(kind, "generation.requested");
		assert_eq!(
			data,
			json!({"id":Uuid::from_u128(1),"task_id":Uuid::from_u128(2),"policy_id":"policy","status":self.created.as_ref().unwrap().status})
		);
		self.point("event").await?;
		self.effects.push("event".into());
		Ok(())
	}
	async fn visible(&mut self, _job: &Request) -> Result<bool> {
		self.point("visible").await?;
		Ok(self.visible)
	}
}
#[async_trait]
impl GenerationAssignmentScope for Scope {
	fn context(&mut self, value: Value) {
		self.context = value;
		self.state.lock().unwrap().calls.push("context".into());
	}
	fn replace_subjects(&mut self, subjects: Vec<String>) {
		self.chain = subjects;
		self.state
			.lock()
			.unwrap()
			.calls
			.push(format!("subjects:{}", self.chain.join("+")));
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		Resource {
			kind: kind.into(),
			id: id.into(),
			tenant: "tenant".into(),
			attributes,
		}
	}
	async fn task(&mut self, id: Uuid) -> Result<Task> {
		assert_eq!(id, self.task.id);
		self.point("task.lock").await?;
		Ok(self.task.clone())
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		assert_eq!(id, self.task.workspace_id);
		self.point("workspace").await?;
		Ok(self.resource(
			"workspace",
			&id.to_string(),
			json!({"workspace_id":id,"team":"research"}),
		))
	}
	async fn inherit_task_origin(&mut self, _id: Uuid) -> Result<bool> {
		self.point("origin").await?;
		Ok(self.inherited)
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.point("task.resource").await?;
		Ok(self.resource(
			"task",
			&task.id.to_string(),
			json!({"created_by":task.created_by}),
		))
	}
	async fn decide(&mut self, _resource: &Resource, action: &str) -> Result<bool> {
		let point = format!("decide:{action}:{}", self.chain.join("+"));
		self.point(&point).await?;
		Ok(!self.state.lock().unwrap().denied.contains(&point))
	}
	async fn policy(&mut self, id: &str) -> Result<Policy> {
		assert_eq!(id, self.policy.id);
		self.point("policy.lock").await?;
		Ok(self.policy.clone())
	}
	async fn existing(&mut self, _task_id: Uuid) -> Result<Option<Request>> {
		self.point("existing").await?;
		Ok(self.existing.clone())
	}
	async fn existing_run(
		&mut self,
		_task_id: Uuid,
	) -> Result<Option<(String, String, Vec<String>)>> {
		self.point("existing.run").await?;
		Ok(self.run.clone())
	}
	async fn catalog(&mut self, search: &Search) -> Result<Vec<Entry>> {
		assert_eq!(search.kind.as_deref(), Some("agent"));
		self.point("catalog.list").await?;
		Ok(self.catalog.clone())
	}
	async fn generated(&mut self, entry: &Entry) -> Result<bool> {
		self.point(format!("generated:{}", entry.id)).await?;
		Ok(self.generated.contains(&entry.id))
	}
	async fn delegate(&mut self, task_id: Uuid, agent: &EntityRef) -> Result<Delegation> {
		self.point("delegate").await?;
		assert_eq!(self.chain, ["alice"]);
		self.effects.push("delegate".into());
		Ok(Delegation {
			task_id,
			node_id: self.node_id().into(),
			agent_id: agent.id.clone(),
			agent_version: agent.version.clone(),
			delivered: true,
		})
	}
}
fn generated(result: Assignment) -> Request {
	match result {
		Assignment::Generated { generation } => *generation,
		_ => panic!("generated assignment required"),
	}
}
#[rstest]
#[tokio::test]
async fn local_creation_locks_before_reservation_and_notifies_after_commit(
	validation: DefinitionValidation,
) {
	let repo = Repository::new();
	let job = generated(
		assign(&repo, Uuid::from_u128(2), "policy", "work", &validation)
			.await
			.unwrap(),
	);
	assert_eq!(job.status, "PENDING_APPROVAL");
	assert_eq!(job.depth, 1);
	assert_eq!(job.token_limit, 200000);
	assert_eq!(job.agent_id, "generated-00000000000000000000000000000001");
	assert_eq!(job.definition["tags"], json!(["generated"]));
	let state = repo.scope.state.lock().unwrap();
	assert_eq!(
		(
			state.commits,
			state.rollbacks,
			state.notifications,
			state.active
		),
		(1, 0, 1, 0)
	);
	assert_eq!(
		state.effects,
		[
			"insert",
			"allocate:200000:0:0",
			"budget:0:0",
			"history:PENDING_APPROVAL",
			"event"
		]
	);
	assert_eq!(
		&state.calls[..11],
		[
			"begin",
			"task.lock",
			"workspace",
			"context",
			"origin",
			"decide:workspace.read:alice",
			"task.resource",
			"decide:task.read:alice",
			"decide:task.delegate:alice",
			"decide:generation.request:alice",
			"decide:generation.read:alice"
		]
	);
	assert!(
		state.calls.iter().position(|c| c == "policy.lock")
			< state.calls.iter().position(|c| c == "existing")
	);
	assert_eq!(
		&state.calls[state.calls.len() - 3..],
		["visible", "finish", "notify"]
	);
}
#[rstest]
#[case(" ")]
#[case("")]
#[case(&"x".repeat(4097))]
#[tokio::test]
async fn invalid_reason_never_loads_a_task(validation: DefinitionValidation, #[case] reason: &str) {
	let mut scope = Scope::new();
	assert!(
		assign_in(
			&mut scope,
			Uuid::from_u128(2),
			"policy",
			reason,
			&validation
		)
		.await
		.is_err()
	);
	assert!(scope.calls().is_empty());
}
#[rstest]
#[case(false, false)]
#[case(true, false)]
#[case(false, true)]
#[tokio::test]
async fn task_origin_rejects_wrong_creator_or_unproven_chain(
	validation: DefinitionValidation,
	#[case] wrong_creator: bool,
	#[case] narrowed: bool,
) {
	let mut scope = Scope::new();
	if wrong_creator {
		scope.task.created_by = "bob".into();
	}
	if narrowed {
		scope.chain.push("ancestor".into());
	}
	let result = assign_in(
		&mut scope,
		Uuid::from_u128(2),
		"policy",
		"work",
		&validation,
	)
	.await;
	if wrong_creator || narrowed {
		assert!(matches!(result, Err(Error::Forbidden)));
		assert!(!scope.calls().iter().any(|c| c == "policy.lock"));
	} else {
		assert!(result.is_ok())
	}
}
#[rstest]
#[tokio::test]
async fn inherited_worker_origin_preserves_the_narrowed_chain(validation: DefinitionValidation) {
	let mut scope = Scope::new();
	scope.inherited = true;
	scope.chain.push("ancestor".into());
	scope.task.created_by = "ancestor".into();
	let job = generated(
		assign_in(
			&mut scope,
			Uuid::from_u128(2),
			"policy",
			"work",
			&validation,
		)
		.await
		.unwrap(),
	);
	assert_eq!(job.subject_chain, ["alice", "ancestor"]);
	assert_eq!(scope.context["team"], "research");
}
#[rstest]
#[case("workspace.read")]
#[case("task.read")]
#[case("task.delegate")]
#[case("generation.request")]
#[case("generation.read")]
#[tokio::test]
async fn current_admission_denials_prevent_policy_and_quota_access(
	validation: DefinitionValidation,
	#[case] action: &str,
) {
	let repo = Repository::new();
	repo.scope
		.state
		.lock()
		.unwrap()
		.denied
		.insert(format!("decide:{action}:alice"));
	assert!(matches!(
		assign(&repo, Uuid::from_u128(2), "policy", "work", &validation).await,
		Err(Error::Forbidden)
	));
	let state = repo.scope.state.lock().unwrap();
	assert_eq!(
		(state.commits, state.rollbacks, state.notifications),
		(0, 1, 0)
	);
	assert!(!state.calls.iter().any(|c| c == "policy.lock"));
	assert!(state.effects.is_empty());
}
#[rstest]
#[case("tenant")]
#[case("subject")]
#[case("policy")]
#[case("reason")]
#[case("chain")]
#[tokio::test]
async fn mismatched_generation_retry_is_a_conflict(
	validation: DefinitionValidation,
	#[case] field: &str,
) {
	let mut scope = Scope::new();
	let mut request = test_support::request(1);
	match field {
		"tenant" => request.tenant = "other".into(),
		"subject" => request.root_subject = "bob".into(),
		"policy" => request.policy_id = "other".into(),
		"reason" => request.reason = "different".into(),
		"chain" => request.subject_chain.push("other".into()),
		_ => panic!("unknown field"),
	};
	scope.existing = Some(request);
	assert!(matches!(
		assign_in(
			&mut scope,
			Uuid::from_u128(2),
			"policy",
			"work",
			&validation
		)
		.await,
		Err(Error::Conflict(_))
	));
	assert!(
		!scope
			.calls()
			.iter()
			.any(|c| c == "visible" || c == "insert")
	);
}
#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn exact_generation_retry_checks_current_visibility_without_spending_quota(
	validation: DefinitionValidation,
	#[case] visible: bool,
) {
	let mut scope = Scope::new();
	scope.existing = Some(test_support::request(1));
	scope.visible = visible;
	let result = assign_in(
		&mut scope,
		Uuid::from_u128(2),
		"policy",
		"work",
		&validation,
	)
	.await;
	if visible {
		assert_eq!(generated(result.unwrap()).id, Uuid::from_u128(1));
	} else {
		assert!(matches!(result, Err(Error::Forbidden)));
	}
	assert_eq!(scope.calls().last().unwrap(), "visible");
	assert!(scope.effects.is_empty());
}
#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn assigned_task_requires_the_exact_chain_and_live_agent_authorization(
	validation: DefinitionValidation,
	#[case] exact: bool,
) {
	let mut scope = Scope::new();
	scope.task.status = TaskStatus::Running;
	let mut chain = vec![
		"alice".into(),
		qualified_agent(scope.node_id(), "ordinary", "1.0.0"),
	];
	if !exact {
		chain[0] = "bob".into();
	}
	scope.run = Some(("ordinary".into(), "1.0.0".into(), chain.clone()));
	let result = assign_in(
		&mut scope,
		Uuid::from_u128(2),
		"policy",
		"work",
		&validation,
	)
	.await;
	if exact {
		let Assignment::Existing { delegation } = result.unwrap() else {
			panic!("existing")
		};
		assert!(delegation.delivered);
		assert_eq!(scope.chain, chain);
		assert_eq!(
			scope.calls().last().unwrap(),
			"catalog:ordinary:agent.execute"
		);
	} else {
		assert!(matches!(result, Err(Error::Conflict(_))));
	}
	assert!(scope.effects.is_empty());
}
#[rstest]
#[tokio::test]
async fn ordinary_agent_reuse_skips_generated_disabled_and_wrong_kind_candidates(
	validation: DefinitionValidation,
) {
	let mut scope = Scope::new();
	scope.ordinary("generated", true, SubjectKind::Agent);
	scope.generated.insert("generated".into());
	scope.ordinary("disabled", false, SubjectKind::Agent);
	scope.ordinary("service", true, SubjectKind::Service);
	scope.ordinary("ordinary", true, SubjectKind::Agent);
	let Assignment::Existing { delegation } = assign_in(
		&mut scope,
		Uuid::from_u128(2),
		"policy",
		"work",
		&validation,
	)
	.await
	.unwrap() else {
		panic!("existing")
	};
	assert_eq!(delegation.agent_id, "ordinary");
	assert_eq!(scope.chain, ["alice"]);
	assert_eq!(scope.effects, ["delegate"]);
	assert!(!scope.calls().iter().any(|c| c == "insert"));
}
#[rstest]
#[case("workspace.read")]
#[case("agent.execute")]
#[case("task.read")]
#[case("task.execute")]
#[tokio::test]
async fn narrowed_candidate_denial_restores_the_chain_and_continues_generation(
	validation: DefinitionValidation,
	#[case] action: &str,
) {
	let mut scope = Scope::new();
	scope.ordinary("ordinary", true, SubjectKind::Agent);
	scope.state.lock().unwrap().denied.insert(format!(
		"decide:{action}:alice+aidash://local/agents/ordinary@1.0.0"
	));
	let job = generated(
		assign_in(
			&mut scope,
			Uuid::from_u128(2),
			"policy",
			"work",
			&validation,
		)
		.await
		.unwrap(),
	);
	assert_eq!(job.subject_chain, ["alice"]);
	assert_eq!(scope.chain, ["alice"]);
	assert!(!scope.calls().iter().any(|c| c == "delegate"));
}
#[rstest]
#[tokio::test]
async fn candidate_cancellation_restores_a_borrowed_worker_chain(validation: DefinitionValidation) {
	let mut scope = Scope::new();
	scope.ordinary("ordinary", true, SubjectKind::Agent);
	scope.state.lock().unwrap().pause =
		Some("decide:task.execute:alice+aidash://local/agents/ordinary@1.0.0".into());
	assert!(
		tokio::time::timeout(
			Duration::from_millis(20),
			assign_in(
				&mut scope,
				Uuid::from_u128(2),
				"policy",
				"work",
				&validation
			)
		)
		.await
		.is_err()
	);
	assert_eq!(scope.chain, ["alice"]);
	assert_eq!(scope.calls().last().unwrap(), "subjects:alice");
	assert!(scope.effects.is_empty());
}
#[rstest]
#[case("insert")]
#[case("allocate")]
#[case("budget")]
#[case("history")]
#[case("event")]
#[case("visible")]
#[case("commit")]
#[tokio::test]
async fn reservation_failures_never_commit_or_notify(
	validation: DefinitionValidation,
	#[case] operation: &str,
) {
	let repo = Repository::new();
	repo.scope.state.lock().unwrap().failure = Some(operation.into());
	assert!(
		assign(&repo, Uuid::from_u128(2), "policy", "work", &validation)
			.await
			.is_err()
	);
	let state = repo.scope.state.lock().unwrap();
	assert_eq!(
		(
			state.commits,
			state.rollbacks,
			state.notifications,
			state.active
		),
		(0, 1, 0, 0)
	);
	assert!(state.effects.is_empty());
}
#[rstest]
#[case("allocate")]
#[case("event")]
#[tokio::test]
async fn cancellation_rolls_back_the_entire_owned_reservation(
	validation: DefinitionValidation,
	#[case] operation: &str,
) {
	let repo = Repository::new();
	repo.scope.state.lock().unwrap().pause = Some(operation.into());
	assert!(
		tokio::time::timeout(
			Duration::from_millis(20),
			assign(&repo, Uuid::from_u128(2), "policy", "work", &validation)
		)
		.await
		.is_err()
	);
	let state = repo.scope.state.lock().unwrap();
	assert_eq!(
		(
			state.commits,
			state.rollbacks,
			state.notifications,
			state.active
		),
		(0, 1, 0, 0)
	);
	assert!(state.effects.is_empty());
}
#[rstest]
#[tokio::test]
async fn foreign_creation_inherits_depth_caps_expiry_and_omits_local_events(
	validation: DefinitionValidation,
) {
	use aidash_domain::generation::remote::Ancestor;
	let mut scope = Scope::new();
	scope.depth = Some(1);
	scope.policy.spec.limits.max_depth = 4;
	let intent = Intent {
		id: Uuid::from_u128(6),
		home_node: "aidash://home".into(),
		source_tenant: "home".into(),
		source_subject: "alice".into(),
		task: scope.task.clone(),
		target_node: "aidash://local".into(),
		policy_id: "policy".into(),
		policy_revision: 7,
		lineage: vec![Ancestor {
			node_id: "aidash://home".into(),
			tenant: "home".into(),
			request_id: Uuid::from_u128(7),
			policy_id: "home-policy".into(),
			policy_revision: 3,
			depth: 3,
			expires_at: scope.foreign_clock + chrono::Duration::seconds(60),
		}],
		reason: "work".into(),
		ttl_seconds: 60,
		expires_at: scope.foreign_clock + chrono::Duration::seconds(60),
	};
	let task = scope.task.clone();
	let policy = scope.policy.clone();
	let job = generated(
		create_in(
			&mut scope,
			&task,
			policy,
			"work",
			Some(&intent),
			&validation,
		)
		.await
		.unwrap(),
	);
	assert_eq!(job.depth, 4);
	assert_eq!(job.home_node, "aidash://home");
	assert_eq!(job.expires_at, intent.expires_at);
	assert_eq!(job.foreign_intent, Some(json!(intent)));
	assert!(!scope.calls().iter().any(|c| c == "event"));
	assert_eq!(scope.effects.last().unwrap(), "history:PENDING_APPROVAL");
}
#[rstest]
#[tokio::test]
async fn task_requirements_mismatch_fails_before_any_reservation(validation: DefinitionValidation) {
	let mut scope = Scope::new();
	scope.task.requirements = json!({"capability":"unavailable"});
	assert!(matches!(
		assign_in(
			&mut scope,
			Uuid::from_u128(2),
			"policy",
			"work",
			&validation
		)
		.await,
		Err(Error::Invalid(_))
	));
	assert!(scope.effects.is_empty());
}
#[rstest]
#[tokio::test]
async fn disabled_policy_cannot_create_after_live_checks(validation: DefinitionValidation) {
	let mut scope = Scope::new();
	scope.policy.spec.enabled = false;
	assert!(matches!(
		assign_in(
			&mut scope,
			Uuid::from_u128(2),
			"policy",
			"work",
			&validation
		)
		.await,
		Err(Error::Forbidden)
	));
	assert!(!scope.calls().iter().any(|c| c == "depth" || c == "insert"));
}
#[rstest]
#[tokio::test]
async fn approval_free_policy_is_queued_and_generated_tag_is_not_duplicated(
	validation: DefinitionValidation,
) {
	let mut scope = Scope::new();
	scope.policy.spec.approval_required = false;
	scope.policy.spec.template.tags.push("generated".into());
	let job = generated(
		assign_in(
			&mut scope,
			Uuid::from_u128(2),
			"policy",
			"work",
			&validation,
		)
		.await
		.unwrap(),
	);
	assert_eq!(job.status, "QUEUED");
	assert_eq!(job.definition["tags"], json!(["generated"]));
}
#[rstest]
#[case("count")]
#[case("concurrency")]
#[case("depth")]
#[case("chain")]
#[case("tokens")]
#[case("tokens_overflow")]
#[case("compaction")]
#[case("embedding")]
#[case("compaction_overflow")]
#[case("embedding_overflow")]
#[tokio::test]
async fn each_quota_limit_is_independent_and_prevents_insert(
	validation: DefinitionValidation,
	#[case] limit: &str,
) {
	use aidash_domain::generation::policy::{Compaction, Embedding};
	let mut scope = Scope::new();
	let provider = EntityRef {
		id: "provider".into(),
		version: "1.0.0".into(),
	};
	match limit {
		"count" => scope.policy.generated_count = 4,
		"concurrency" => scope.active = 2,
		"depth" => scope.depth = Some(2),
		"chain" => {
			scope.inherited = true;
			scope.chain.resize(32, "ancestor".into());
		}
		"tokens" => scope.policy.allocated_tokens = 600001,
		"tokens_overflow" => scope.policy.allocated_tokens = i64::MAX,
		"compaction" | "compaction_overflow" => {
			scope.policy.spec.compaction = Some(Compaction {
				provider,
				calls_per_agent: 2,
				call_budget: 3,
			});
			scope.policy.allocated_compaction_calls = if limit.ends_with("overflow") {
				i64::MAX
			} else {
				2
			};
		}
		"embedding" | "embedding_overflow" => {
			scope.policy.spec.embedding = Some(Embedding {
				provider,
				calls_per_agent: 2,
				call_budget: 3,
			});
			scope.policy.allocated_embedding_calls = if limit.ends_with("overflow") {
				i64::MAX
			} else {
				2
			};
		}
		_ => panic!("unknown quota"),
	};
	assert!(matches!(
		assign_in(
			&mut scope,
			Uuid::from_u128(2),
			"policy",
			"work",
			&validation
		)
		.await,
		Err(Error::Domain(aidash_domain::Error::Conflict(_)))
	));
	assert!(!scope.calls().iter().any(|c| c == "insert"));
	assert!(scope.effects.is_empty());
}
