use super::*;
use crate::generation::test_support;
use aidash_domain::{
	TaskStatus,
	federation::execution::Inspection,
	generation::{
		intent::guards::{Authority, Record},
		remote::Ancestor,
		requests::Request,
	},
	policy::Resource,
	registry::Entry,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::{fixture, rstest};

struct Scope {
	authority: Authority,
	task: Task,
	intent: Intent,
	description: Description,
	job: Option<Request>,
	record: Option<Record>,
	lineage: Vec<Ancestor>,
	now: DateTime<Utc>,
	live: bool,
	events: Vec<&'static str>,
	required: Vec<(Resource, String)>,
	denied: Option<String>,
	failure: Option<&'static str>,
	pause: Option<&'static str>,
	bound: Vec<(Uuid, Uuid, Uuid)>,
	activations: Vec<Uuid>,
}
#[fixture]
fn scope() -> Scope {
	let now = DateTime::<Utc>::from_timestamp(1000, 0).unwrap();
	let task = Task {
		id: Uuid::from_u128(1),
		workspace_id: Uuid::from_u128(2),
		title: "Task".into(),
		description: "Work".into(),
		status: TaskStatus::Open,
		requirements: json!({}),
		owner: None,
		created_by: "root".into(),
		dependencies: vec![],
		parent_id: None,
		revision: 7,
		created_at: now,
	};
	let authority = Authority {
		tenant: "tenant".into(),
		credential_id: Uuid::from_u128(3),
		subjects: vec!["root".into(), "aidash://executor/agents/agent@1.0.0".into()],
	};
	let intent = Intent {
		id: Uuid::from_u128(4),
		home_node: "aidash://home".into(),
		source_tenant: "tenant".into(),
		source_subject: "root".into(),
		task: task.clone(),
		target_node: "aidash://executor".into(),
		policy_id: "policy".into(),
		policy_revision: 4,
		lineage: vec![],
		reason: "Generate".into(),
		ttl_seconds: 1000,
		expires_at: DateTime::from_timestamp(2000, 0).unwrap(),
	};
	let agent: Entry =
		serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent",
        "name":{"en":"Agent"},"description":{}}))
		.unwrap();
	let description = Description {
		grant_id: Uuid::from_u128(5),
		source_node: intent.home_node.clone(),
		target_node: intent.target_node.clone(),
		source_tenant: intent.source_tenant.clone(),
		source_subject: intent.source_subject.clone(),
		task: task.clone(),
		inspection: Inspection {
			generation: Some(json!(intent)),
			lineage: vec![],
			node_id: intent.target_node.clone(),
			authority_digest: "pinned".into(),
			agent,
			definitions: vec![],
			semantic_memory: 0,
			compactor: None,
		},
		expires_at: intent.expires_at,
		semantic: Default::default(),
	};
	let record = Record {
		tenant: authority.tenant.clone(),
		credential_id: authority.credential_id,
		root_subject: "root".into(),
		subject_chain: vec!["root".into()],
		binding: json!(intent),
		cancelled: false,
	};
	let mut job = test_support::request(11);
	job.task_id = task.id;
	job.home_node = intent.home_node.clone();
	job.tenant = authority.tenant.clone();
	job.credential_id = authority.credential_id;
	job.subject_chain = authority.subjects.clone();
	job.agent_id = "agent".into();
	job.agent_version = "1.0.0".into();
	job.prepared = true;
	job.status = "QUEUED".into();
	job.expires_at = intent.expires_at;
	job.foreign_intent = Some(json!(intent));
	job.grant_id = None;
	job.admission_id = None;
	Scope {
		authority,
		task,
		intent,
		description,
		job: Some(job),
		record: Some(record),
		lineage: vec![],
		now,
		live: true,
		events: vec![],
		required: vec![],
		denied: None,
		failure: None,
		pause: None,
		bound: vec![],
		activations: vec![],
	}
}
impl Scope {
	async fn point(&mut self, name: &'static str) -> Result<()> {
		self.events.push(name);
		if self.failure == Some(name) {
			return Err(Error::External(format!("{name} failed")));
		}
		if self.pause == Some(name) {
			std::future::pending::<()>().await;
		}
		Ok(())
	}
}
#[async_trait]
impl ForeignGenerationGuard for Scope {
	fn authority(&self) -> Authority {
		self.authority.clone()
	}
	fn now(&self) -> DateTime<Utc> {
		self.now
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		Resource {
			tenant: self.authority.tenant.clone(),
			kind: kind.into(),
			id: id.into(),
			attributes,
		}
	}
	async fn job(&mut self, agent: &EntityRef) -> Result<Option<Request>> {
		assert_eq!((&*agent.id, &*agent.version), ("agent", "1.0.0"));
		self.point("job").await?;
		Ok(self.job.clone())
	}
	async fn intent(&mut self, id: Uuid) -> Result<Option<Record>> {
		assert_eq!(id, self.intent.id);
		self.point("intent").await?;
		Ok(self.record.clone())
	}
	async fn lineage(&mut self, node: &str) -> Result<Vec<Ancestor>> {
		assert_eq!(node, "aidash://home");
		self.point("lineage").await?;
		Ok(self.lineage.clone())
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		assert_eq!(id, self.task.workspace_id);
		self.point("workspace").await?;
		Ok(self.resource("workspace", &id.to_string(), json!({})))
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		assert_eq!(task.id, self.task.id);
		self.point("task.resource").await?;
		Ok(self.resource(
			"task",
			&task.id.to_string(),
			json!({"workspace_id":task.workspace_id}),
		))
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.required.push((resource.clone(), action.into()));
		self.point("require").await?;
		if self.denied.as_deref() == Some(action) {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn active(&mut self, description: &Description, admission: Uuid) -> Result<bool> {
		assert_eq!(description.grant_id, self.description.grant_id);
		assert_eq!(admission, Uuid::from_u128(6));
		self.point("active").await?;
		Ok(self.live)
	}
}
#[async_trait]
impl ForeignGenerationBinding for Scope {
	fn now(&self) -> DateTime<Utc> {
		self.now
	}
	async fn prepared(&mut self, home: &str, task: Uuid) -> Result<Request> {
		assert_eq!(home, "aidash://home");
		assert_eq!(task, self.task.id);
		self.point("prepared").await?;
		Ok(self.job.clone().unwrap())
	}
	async fn bind(&mut self, job: Uuid, grant: Uuid, admission: Uuid) -> Result<()> {
		self.point("bind").await?;
		self.bound.push((job, grant, admission));
		Ok(())
	}
	async fn activate(&mut self, job: &Request) -> Result<()> {
		assert_eq!(job.status, "QUEUED");
		self.point("activate").await?;
		self.activations.push(job.id);
		Ok(())
	}
}
fn agent() -> EntityRef {
	EntityRef {
		id: "agent".into(),
		version: "1.0.0".into(),
	}
}

#[rstest]
#[case("QUEUED")]
#[case("ACTIVE")]
#[tokio::test]
async fn prepared_executor_inspection_returns_only_its_exact_foreign_intent(
	mut scope: Scope,
	#[case] status: &str,
) {
	scope.job.as_mut().unwrap().status = status.into();
	let expected = json!(scope.intent);
	let task = scope.task.id;
	assert_eq!(
		inspect(&mut scope, "aidash://home", Some(task), &agent())
			.await
			.unwrap(),
		Some(expected)
	);
	assert_eq!(scope.events, ["job"]);
}
#[rstest]
#[case("home")]
#[case("task")]
#[case("tenant")]
#[case("credential")]
#[case("chain")]
#[case("prepared")]
#[case("status")]
#[case("expiry")]
#[tokio::test]
async fn each_executor_authority_or_liveness_change_is_forbidden(
	mut scope: Scope,
	#[case] changed: &str,
) {
	let job = scope.job.as_mut().unwrap();
	match changed {
		"home" => job.home_node = "aidash://other".into(),
		"task" => job.task_id = Uuid::from_u128(99),
		"tenant" => job.tenant = "other".into(),
		"credential" => job.credential_id = Uuid::from_u128(99),
		"chain" => job.subject_chain.reverse(),
		"prepared" => job.prepared = false,
		"status" => job.status = "COMPLETED".into(),
		"expiry" => job.expires_at = scope.now,
		_ => panic!("case"),
	}
	let task = scope.task.id;
	assert!(matches!(
		inspect(&mut scope, "aidash://home", Some(task), &agent()).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.events, ["job"]);
}
#[rstest]
#[tokio::test]
async fn ordinary_agents_and_missing_foreign_tasks_preserve_their_existing_boundaries(
	mut scope: Scope,
) {
	let task = scope.task.id;
	scope.job = None;
	assert_eq!(
		inspect(&mut scope, "aidash://home", Some(task), &agent())
			.await
			.unwrap(),
		None
	);
	let mut scope = crate::generation::foreign::tests::scope();
	assert!(matches!(
		inspect(&mut scope, "aidash://home", None, &agent()).await,
		Err(Error::Forbidden)
	));
}
#[rstest]
#[case("QUEUED", false, false)]
#[case("QUEUED", true, true)]
#[case("ACTIVE", true, false)]
#[tokio::test]
async fn admission_binding_is_exact_and_activates_only_a_queued_request(
	mut scope: Scope,
	#[case] status: &str,
	#[case] activate: bool,
	#[case] transitioned: bool,
) {
	scope.job.as_mut().unwrap().status = status.into();
	let description = scope.description.clone();
	bind(&mut scope, &description, Uuid::from_u128(6), activate)
		.await
		.unwrap();
	assert_eq!(
		scope.bound,
		vec![(
			Uuid::from_u128(11),
			description.grant_id,
			Uuid::from_u128(6)
		)]
	);
	assert_eq!(
		scope.activations,
		if transitioned {
			vec![Uuid::from_u128(11)]
		} else {
			vec![]
		}
	);
}
#[rstest]
#[case("intent")]
#[case("intent_task")]
#[case("agent")]
#[case("version")]
#[case("grant")]
#[case("admission")]
#[case("prepared")]
#[case("status")]
#[case("expiry")]
#[tokio::test]
async fn foreign_binding_fences_precede_every_mutation(mut scope: Scope, #[case] changed: &str) {
	let job = scope.job.as_mut().unwrap();
	match changed {
		"intent" => job.foreign_intent = None,
		"intent_task" => {
			let mut intent = scope.intent.clone();
			intent.task.id = Uuid::from_u128(99);
			scope.description.inspection.generation = Some(json!(intent));
			job.foreign_intent = Some(json!(intent));
		}
		"agent" => job.agent_id = "other".into(),
		"version" => job.agent_version = "2.0.0".into(),
		"grant" => job.grant_id = Some(Uuid::from_u128(99)),
		"admission" => job.admission_id = Some(Uuid::from_u128(99)),
		"prepared" => job.prepared = false,
		"status" => job.status = "PENDING_APPROVAL".into(),
		"expiry" => job.expires_at = scope.now,
		_ => panic!("case"),
	}
	let description = scope.description.clone();
	assert!(matches!(
		bind(&mut scope, &description, Uuid::from_u128(6), true).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.events, ["prepared"]);
	assert_eq!(scope.bound, vec![]);
	assert_eq!(scope.activations, Vec::<Uuid>::new());
}
#[rstest]
#[tokio::test]
async fn already_bound_grant_and_admission_are_idempotent(mut scope: Scope) {
	scope.job.as_mut().unwrap().grant_id = Some(scope.description.grant_id);
	scope.job.as_mut().unwrap().admission_id = Some(Uuid::from_u128(6));
	let description = scope.description.clone();
	bind(&mut scope, &description, Uuid::from_u128(6), false)
		.await
		.unwrap();
	assert_eq!(scope.bound.len(), 1);
	assert_eq!(scope.activations.len(), 0);
}
#[rstest]
#[tokio::test]
async fn home_guard_holds_the_saved_binding_then_requires_all_current_permissions(
	mut scope: Scope,
) {
	let task = scope.task.clone();
	let generation = json!(scope.intent);
	check_home(&mut scope, &task, "aidash://executor", Some(&generation))
		.await
		.unwrap();
	assert_eq!(
		scope.events,
		[
			"intent",
			"lineage",
			"workspace",
			"require",
			"require",
			"task.resource",
			"require",
			"require",
			"require",
			"require"
		]
	);
	assert_eq!(
		scope
			.required
			.iter()
			.map(|(_, action)| action.as_str())
			.collect::<Vec<_>>(),
		[
			"workspace.read",
			"generation.disclose",
			"task.read",
			"task.delegate",
			"federation.execute",
			"generation.request"
		]
	);
	assert_eq!(scope.required[4].0.id, "aidash://executor");
	assert_eq!(
		scope.required[5].0.id,
		"aidash://executor/generation-policies/policy"
	);
	assert_eq!(
		scope.required[5].0.attributes,
		json!({"remote_node":"aidash://executor"})
	);
}
#[rstest]
#[case("cancelled")]
#[case("binding")]
#[case("credential")]
#[case("tenant")]
#[case("chain")]
#[case("node")]
#[case("task")]
#[case("workspace")]
#[case("expiry")]
#[tokio::test]
async fn changed_home_binding_cannot_reach_lineage_or_permission_queries(
	mut scope: Scope,
	#[case] changed: &str,
) {
	let mut intent = scope.intent.clone();
	let record = scope.record.as_mut().unwrap();
	match changed {
		"cancelled" => record.cancelled = true,
		"binding" => record.binding = Value::Null,
		"credential" => record.credential_id = Uuid::from_u128(99),
		"tenant" => record.tenant = "other".into(),
		"chain" => record.subject_chain = scope.authority.subjects.clone(),
		"node" => intent.target_node = "aidash://other".into(),
		"task" => intent.task.id = Uuid::from_u128(99),
		"workspace" => intent.task.workspace_id = Uuid::from_u128(99),
		"expiry" => intent.expires_at = scope.now,
		_ => panic!("case"),
	}
	let generation = json!(intent);
	if matches!(changed, "node" | "task" | "workspace" | "expiry") {
		record.binding = generation.clone();
	}
	let task = scope.task.clone();
	assert!(matches!(
		check_home(&mut scope, &task, "aidash://executor", Some(&generation)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.events, ["intent"]);
	assert_eq!(scope.required.len(), 0);
}
#[rstest]
#[tokio::test]
async fn missing_or_changed_home_lineage_never_uses_current_permissions(mut scope: Scope) {
	let task = scope.task.clone();
	let generation = json!(scope.intent);
	scope.record = None;
	assert!(matches!(
		check_home(&mut scope, &task, "aidash://executor", Some(&generation)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.events, ["intent"]);
	let mut scope = crate::generation::foreign::tests::scope();
	scope.lineage.push(Ancestor {
		node_id: "aidash://home".into(),
		tenant: "tenant".into(),
		request_id: Uuid::from_u128(88),
		policy_id: "policy".into(),
		policy_revision: 2,
		depth: 1,
		expires_at: scope.intent.expires_at,
	});
	assert!(matches!(
		check_home(&mut scope, &task, "aidash://executor", Some(&generation)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.events, ["intent", "lineage"]);
	assert_eq!(scope.required.len(), 0);
}
#[rstest]
#[case("workspace.read", 1)]
#[case("generation.disclose", 2)]
#[case("task.read", 3)]
#[case("task.delegate", 4)]
#[case("federation.execute", 5)]
#[case("generation.request", 6)]
#[tokio::test]
async fn current_home_denial_stops_at_the_exact_action(
	mut scope: Scope,
	#[case] action: &str,
	#[case] count: usize,
) {
	scope.denied = Some(action.into());
	let task = scope.task.clone();
	let generation = json!(scope.intent);
	assert!(matches!(
		check_home(&mut scope, &task, "aidash://executor", Some(&generation)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.required.len(), count);
	assert_eq!(scope.required.last().unwrap().1, action);
}
#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn active_generation_uses_the_exact_database_liveness_predicate(
	mut scope: Scope,
	#[case] live: bool,
) {
	scope.live = live;
	let description = scope.description.clone();
	let result = require_active(&mut scope, &description, Uuid::from_u128(6)).await;
	if live {
		result.unwrap();
	} else {
		assert!(matches!(result, Err(Error::Forbidden)));
	}
	assert_eq!(scope.events, ["active"]);
}
#[rstest]
#[tokio::test]
async fn ordinary_execution_skips_foreign_only_guards(mut scope: Scope) {
	let mut description = scope.description.clone();
	description.inspection.generation = None;
	bind(&mut scope, &description, Uuid::from_u128(6), true)
		.await
		.unwrap();
	require_active(&mut scope, &description, Uuid::from_u128(6))
		.await
		.unwrap();
	let task = scope.task.clone();
	check_home(&mut scope, &task, "aidash://executor", None)
		.await
		.unwrap();
	check_preparation(&task, None).unwrap();
	assert_eq!(scope.events, Vec::<&str>::new());
}
#[rstest]
#[case("task")]
#[case("revision")]
fn preparation_requires_the_same_task_and_revision(scope: Scope, #[case] changed: &str) {
	let mut task = scope.task.clone();
	check_preparation(&task, Some(&json!(scope.intent))).unwrap();
	if changed == "task" {
		task.id = Uuid::from_u128(99);
	} else {
		task.revision += 1;
	}
	assert!(
		matches!(check_preparation(&task,Some(&json!(scope.intent))),Err(Error::Conflict(message))
        if message=="generation intent task revision changed before grant preparation")
	);
}
#[rstest]
fn malformed_intent_keeps_json_failure_distinct_from_authority_denial(scope: Scope) {
	assert!(matches!(
		check_preparation(&scope.task, Some(&Value::Null)),
		Err(Error::Json(_))
	));
}
#[rstest]
#[tokio::test]
async fn cancellation_before_binding_has_no_provisional_mutation(mut scope: Scope) {
	scope.pause = Some("prepared");
	let description = scope.description.clone();
	assert!(
		tokio::time::timeout(
			std::time::Duration::from_millis(10),
			bind(&mut scope, &description, Uuid::from_u128(6), true)
		)
		.await
		.is_err()
	);
	assert_eq!(scope.events, ["prepared"]);
	assert_eq!(scope.bound, vec![]);
	assert_eq!(scope.activations, Vec::<Uuid>::new());
}
