use super::*;
use crate::ports::authorization::ExecutionGrantSession;
use aidash_domain::{
	identity::execution::{ExecutionPrincipal, TaskOrigin},
	policy::{PolicyBundle, Resource},
	registry::Entry,
};
use async_trait::async_trait;
use rstest::rstest;
use serde_json::{Value, json};
struct Scope {
	task: Option<Task>,
	entry: Entry,
	bundle: PolicyBundle,
	identity: ExecutionPrincipal,
	subjects: Vec<String>,
	context: Value,
	calls: Vec<String>,
	fail: Option<&'static str>,
	deny: Option<&'static str>,
	active: bool,
	origin: Option<TaskOrigin>,
	mapping: bool,
	grants: Vec<ExecutionGrant>,
	claim_args: Option<(i64, String)>,
	bound_origin: Option<(Uuid, Uuid, Uuid)>,
}
impl Scope {
	fn new() -> Self {
		let subject = qualified_agent("aidash://node", "agent", "1.0.0");
		Self{task:Some(Task{id:Uuid::from_u128(1),workspace_id:Uuid::from_u128(2),title:"task".into(),description:String::new(),status:TaskStatus::Open,requirements:json!({}),owner:None,created_by:"root".into(),dependencies:vec![],parent_id:None,revision:7,created_at:chrono::Utc::now()}),entry:serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent","name":{},"description":{},"config":{"schema_version":1,"instructions":"Fixture","bindings":[],"remove_default":[],"model":{"id":"model","version":"1.0.0"}}})).unwrap(),bundle:serde_json::from_value(json!({"tenant":"tenant","subjects":{subject:{"kind":"agent","delegated_by":null}}})).unwrap(),identity:ExecutionPrincipal{tenant:"tenant".into(),subject:"root".into(),credential_id:Uuid::from_u128(4)},subjects:vec!["root".into()],context:json!({}),calls:vec![],fail:None,deny:None,active:true,origin:None,mapping:false,grants:vec![],claim_args:None,bound_origin:None}
	}
	fn call(&mut self, name: &str) -> Result<()> {
		self.calls.push(name.into());
		if self.fail == Some(name) {
			return Err(Error::Port(Box::new(std::io::Error::other(
				"admission adapter fault",
			))));
		}
		if self.deny == Some(name) {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
}
#[async_trait]
impl ExecutionGrantSession for Scope {
	fn identity(&self) -> ExecutionPrincipal {
		self.identity.clone()
	}
	fn subjects(&self) -> &[String] {
		&self.subjects
	}
	fn set_subjects(&mut self, s: Vec<String>) {
		self.subjects = s;
	}
	fn select_worker(&mut self, _: Uuid, _: Option<bool>) {
		panic!("admission does not select a running worker");
	}
	async fn refresh(&mut self, _: Uuid) -> Result<()> {
		panic!("admission does not refresh a running worker");
	}
	async fn required_grant(&mut self, _: Uuid) -> Result<ExecutionGrant> {
		panic!("admission does not read a grant before claim");
	}
	async fn optional_grant(&mut self, _: Uuid) -> Result<Option<ExecutionGrant>> {
		panic!("admission does not read a grant before claim");
	}
	async fn local_origin(&mut self, _: Uuid) -> Result<Option<TaskOrigin>> {
		self.call("local_origin")?;
		Ok(self.origin.clone())
	}
	async fn remote_origin(&mut self, _: Uuid) -> Result<Option<TaskOrigin>> {
		self.call("remote_origin")?;
		Ok(None)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.call("workspace")?;
		Ok(Resource {
			tenant: "tenant".into(),
			kind: "workspace".into(),
			id: id.to_string(),
			attributes: json!({"workspace":"context"}),
		})
	}
	fn set_context(&mut self, c: Value) {
		self.context = c;
	}
	async fn require(&mut self, _: &Resource, action: &str) -> Result<()> {
		self.call(action)
	}
}
#[async_trait]
impl ExecutionAdmissionSession for Scope {
	async fn bindings(
		&mut self,
		entry: &Entry,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
		let _: aidash_domain::registry::bindings::AgentBindings =
			serde_json::from_value(entry.config.clone())?;
		Ok(crate::test_support::resolve(
			self.node_id(),
			entry,
			false,
			vec![],
		))
	}
	fn node_id(&self) -> &str {
		"aidash://node"
	}
	fn bundle(&self) -> &PolicyBundle {
		&self.bundle
	}
	async fn task(&mut self, _: Uuid) -> Result<Option<Task>> {
		self.call("task")?;
		Ok(self.task.clone())
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.call("task_resource")?;
		Ok(Resource {
			tenant: "tenant".into(),
			kind: "task".into(),
			id: task.id.to_string(),
			attributes: json!({}),
		})
	}
	async fn require_live_agent(&mut self, _: Uuid, _: &EntityRef) -> Result<()> {
		self.call("live_agent")
	}
	async fn executable_entry(&mut self, _: &EntityRef) -> Result<Entry> {
		self.call("entry")?;
		Ok(self.entry.clone())
	}
	async fn active_installation(&mut self, _: &Entry) -> Result<bool> {
		self.call("active")?;
		Ok(self.active)
	}
	async fn check_pinned_installation(&mut self, _: &Entry) -> Result<()> {
		self.call("pinned")
	}
	async fn prepare_thread(&mut self, _: &Task, _: &AgentConfig, _: &str) -> Result<Option<Uuid>> {
		self.call("prepare_thread")?;
		Ok(Some(Uuid::from_u128(8)))
	}
	async fn claim(
		&mut self,
		task: &Task,
		revision: i64,
		subject: &str,
		_: &Entry,
		_: &aidash_domain::registry::bindings::BindingSnapshot,
	) -> Result<Task> {
		self.call("claim")?;
		self.claim_args = Some((revision, subject.into()));
		let mut task = task.clone();
		task.status = TaskStatus::Claimed;
		task.owner = Some(subject.into());
		Ok(task)
	}
	async fn claimed_run(&mut self, _: Uuid) -> Result<Uuid> {
		self.call("run")?;
		Ok(Uuid::from_u128(3))
	}
	async fn persist_grant(&mut self, g: &ExecutionGrant) -> Result<()> {
		self.call("grant")?;
		self.grants.push(g.clone());
		Ok(())
	}
	async fn dashboard_origin(&mut self, credential: Uuid) -> Result<Option<(Uuid, Uuid)>> {
		assert_eq!(credential, self.identity.credential_id);
		self.call("mapping")?;
		Ok(self
			.mapping
			.then_some((Uuid::from_u128(5), Uuid::from_u128(6))))
	}
	async fn persist_dashboard_origin(
		&mut self,
		run: Uuid,
		identity: Uuid,
		mapping: Uuid,
	) -> Result<()> {
		self.call("dashboard_origin")?;
		self.bound_origin = Some((run, identity, mapping));
		Ok(())
	}
	async fn admit_thread(
		&mut self,
		_: &Task,
		run: Uuid,
		thread: Option<Uuid>,
		_: &AgentConfig,
		_: &str,
	) -> Result<()> {
		assert_eq!(run, Uuid::from_u128(3));
		assert_eq!(thread, Some(Uuid::from_u128(8)));
		self.call("admit_thread")
	}
	async fn local_delegation(&mut self, task: Uuid, agent: &EntityRef) -> Result<Delegation> {
		self.call("delegation")?;
		Ok(Delegation {
			task_id: task,
			node_id: self.node_id().into(),
			agent_id: agent.id.clone(),
			agent_version: agent.version.clone(),
			delivered: true,
		})
	}
	async fn delegation_event(&mut self, workspace: Uuid, _: &Delegation) -> Result<()> {
		assert_eq!(workspace, Uuid::from_u128(2));
		self.call("event")
	}
}
fn agent() -> EntityRef {
	EntityRef {
		id: "agent".into(),
		version: "1.0.0".into(),
	}
}
#[rstest]
#[case(None, false)]
#[case(Some(3), true)]
#[tokio::test]
async fn admitted_task_retains_credential_chain_revision_and_browser_origin(
	#[case] revision: Option<i64>,
	#[case] browser: bool,
) {
	let mut s = Scope::new();
	s.mapping = browser;
	s.origin = Some(TaskOrigin {
		tenant: "tenant".into(),
		root_subject: "root".into(),
		subject_chain: vec!["root".into(), "delegator".into()],
	});
	let task = admit(&mut s, Uuid::from_u128(1), revision, &agent(), false)
		.await
		.unwrap();
	let qualified = qualified_agent(s.node_id(), "agent", "1.0.0");
	assert_eq!(task.status, TaskStatus::Claimed);
	assert_eq!(
		s.claim_args,
		Some((revision.unwrap_or(7), qualified.clone()))
	);
	assert_eq!(
		s.grants,
		vec![ExecutionGrant {
			run_id: Uuid::from_u128(3),
			task_id: Uuid::from_u128(1),
			workspace_id: Uuid::from_u128(2),
			tenant: "tenant".into(),
			credential_id: Uuid::from_u128(4),
			root_subject: "root".into(),
			subject_chain: vec!["root".into(), "delegator".into(), qualified]
		}]
	);
	assert_eq!(s.context, json!({"workspace":"context"}));
	assert_eq!(
		s.bound_origin,
		browser.then_some((Uuid::from_u128(3), Uuid::from_u128(5), Uuid::from_u128(6)))
	);
	let position = |name: &str| s.calls.iter().position(|call| call == name).unwrap();
	assert!(position("prepare_thread") < position("claim"));
	assert!(position("grant") < position("admit_thread"));
	assert_eq!(
		s.calls
			.iter()
			.filter(|c| c.as_str() == "workspace.read")
			.count(),
		2
	);
}
#[rstest]
#[case(TaskStatus::Claimed)]
#[case(TaskStatus::Running)]
#[case(TaskStatus::Completed)]
#[case(TaskStatus::Failed)]
#[case(TaskStatus::Blocked)]
#[case(TaskStatus::Cancelled)]
#[case(TaskStatus::Abandoned)]
#[tokio::test]
async fn assignment_conflict_is_checked_after_content_authorization(#[case] status: TaskStatus) {
	let mut s = Scope::new();
	s.task.as_mut().unwrap().status = status;
	let e = admit(&mut s, Uuid::from_u128(1), None, &agent(), false)
		.await
		.unwrap_err();
	assert!(matches!(e,Error::Conflict(ref m) if m=="task is already assigned"));
	assert!(s.calls.contains(&"task.read".into()));
	assert!(s.calls.contains(&"workspace.read".into()));
	assert!(!s.calls.contains(&"claim".into()));
}
#[rstest]
#[case("task.read")]
#[case("workspace.read")]
#[case("task.delegate")]
#[case("task.execute")]
#[case("prepare_thread")]
#[tokio::test]
async fn denied_admission_never_claims_or_binds_a_run(#[case] denial: &'static str) {
	let mut s = Scope::new();
	s.deny = Some(denial);
	assert!(matches!(
		admit(&mut s, Uuid::from_u128(1), None, &agent(), true).await,
		Err(Error::Forbidden)
	));
	assert!(s.claim_args.is_none());
	assert!(s.grants.is_empty());
	assert!(!s.calls.contains(&"delegation".into()));
}
#[rstest]
#[case("task")]
#[case("local_origin")]
#[case("remote_origin")]
#[case("workspace")]
#[case("task_resource")]
#[case("live_agent")]
#[case("entry")]
#[case("active")]
#[case("pinned")]
#[case("prepare_thread")]
#[case("claim")]
#[case("run")]
#[case("grant")]
#[case("mapping")]
#[case("admit_thread")]
#[tokio::test]
async fn adapter_failure_is_preserved_and_stops_following_effects(#[case] boundary: &'static str) {
	let mut s = Scope::new();
	s.fail = Some(boundary);
	let e = admit(&mut s, Uuid::from_u128(1), None, &agent(), true)
		.await
		.unwrap_err();
	assert!(matches!(e,Error::Port(ref error) if error.to_string()=="admission adapter fault"));
	assert_eq!(s.calls.last().map(String::as_str), Some(boundary));
}
#[rstest]
#[case("missing_task")]
#[case("agent_subject")]
#[case("inactive")]
#[tokio::test]
async fn missing_authority_cannot_allocate_work(#[case] kind: &str) {
	let mut s = Scope::new();
	match kind {
		"missing_task" => s.task = None,
		"agent_subject" => s.bundle.subjects.clear(),
		"inactive" => s.active = false,
		_ => panic!("invalid case"),
	};
	assert!(matches!(
		admit(&mut s, Uuid::from_u128(1), None, &agent(), false).await,
		Err(Error::Forbidden)
	));
	assert!(s.grants.is_empty());
	assert!(s.claim_args.is_none());
}
#[tokio::test]
async fn delegation_depth_fails_before_catalog_execution_or_claim() {
	let mut s = Scope::new();
	s.subjects = vec!["root".into(); 32];
	let e = admit(&mut s, Uuid::from_u128(1), None, &agent(), false)
		.await
		.unwrap_err();
	assert!(matches!(e,Error::Invalid(ref m) if m=="execution delegation depth exceeds 32"));
	assert!(!s.calls.contains(&"entry".into()));
	assert!(s.claim_args.is_none());
}
#[tokio::test]
async fn persisted_agent_configuration_errors_keep_the_json_error_kind() {
	let mut s = Scope::new();
	s.entry.config = json!({"model":false});
	assert!(matches!(
		admit(&mut s, Uuid::from_u128(1), None, &agent(), false).await,
		Err(Error::Json(_))
	));
	assert!(s.claim_args.is_none());
}
#[tokio::test]
async fn wrong_catalog_kind_keeps_the_existing_invalid_executor_error() {
	let mut s = Scope::new();
	s.entry.kind = "tool".into();
	let e = admit(&mut s, Uuid::from_u128(1), None, &agent(), false)
		.await
		.unwrap_err();
	assert!(matches!(e,Error::Invalid(ref m) if m=="executor must be an agent"));
	assert!(!s.calls.contains(&"pinned".into()));
}
#[tokio::test]
async fn local_delegation_is_audited_only_after_admission_and_grant_binding() {
	let mut s = Scope::new();
	let d = delegate(&mut s, Uuid::from_u128(1), &agent())
		.await
		.unwrap();
	assert!(d.delivered);
	assert_eq!(d.task_id, Uuid::from_u128(1));
	assert_eq!(s.calls[s.calls.len() - 2..], ["delegation", "event"]);
	assert_eq!(s.grants.len(), 1);
	assert!(s.calls.contains(&"task.delegate".into()));
}
