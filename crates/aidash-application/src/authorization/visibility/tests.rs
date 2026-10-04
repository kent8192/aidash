use super::*;
use crate::{Error, ports::authorization::visibility::*};
use aidash_domain::{Conversation, HumanRequest, Task, TaskStatus, policy::Resource};
use async_trait::async_trait;
use rstest::{fixture, rstest};

#[fixture]
fn run() -> RunMetadata {
	RunMetadata {
		id: Uuid::from_u128(1),
		task_id: Uuid::from_u128(2),
		workspace_id: Uuid::from_u128(3),
		home_node: "aidash://node".into(),
		agent_id: "agent".into(),
		agent_version: "1".into(),
		phase: aidash_domain::RunPhase::Ready,
		control: aidash_domain::RunControl::Active,
		step: 0,
		revision: 0,
		observed_input_seq: 0,
		ledger_worker_ready: false,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: chrono::Utc::now(),
	}
}

struct Scope {
	cache: Option<bool>,
	frontier: bool,
	missing_task: bool,
	task_visible: bool,
	humans: bool,
	reads: bool,
	admission: bool,
	legacy: bool,
	foreign: bool,
	denied: Option<&'static str>,
	fail: Option<&'static str>,
	calls: Vec<&'static str>,
	decisions: Vec<Resource>,
}

#[fixture]
fn scope() -> Scope {
	Scope {
		cache: None,
		frontier: false,
		missing_task: false,
		task_visible: true,
		humans: true,
		reads: true,
		admission: false,
		legacy: false,
		foreign: true,
		denied: None,
		fail: None,
		calls: vec![],
		decisions: vec![],
	}
}

impl Scope {
	fn touch(&mut self, name: &'static str) -> Result<()> {
		self.calls.push(name);
		if self.fail == Some(name) {
			return Err(Error::Port(Box::new(std::io::Error::other(
				"read adapter fault",
			))));
		}
		Ok(())
	}
}

#[async_trait]
impl LocalRunVisibilityScope for Scope {
	fn cached(&self, _: &RunMetadata) -> Option<bool> {
		if self.frontier { None } else { self.cache }
	}
	fn remember(&mut self, _: &RunMetadata, allowed: bool) {
		if !self.frontier {
			self.cache = Some(allowed);
		}
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		Resource {
			tenant: "tenant".into(),
			kind: kind.into(),
			id: id.into(),
			attributes,
		}
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.touch("workspace")?;
		Ok(self.resource(
			"workspace",
			&id.to_string(),
			json!({"workspace_id":id,"owner":"root"}),
		))
	}
	async fn memory_resource(
		&mut self,
		run: &RunMetadata,
		workspace: &Resource,
	) -> Result<Resource> {
		self.touch("memory")?;
		Ok(self.resource(
			"memory",
			&run.agent_id,
			memory_attributes(run, workspace.attributes.clone()),
		))
	}
	async fn task(&mut self, run: &RunMetadata) -> Result<Option<Task>> {
		self.touch("task")?;
		Ok((!self.missing_task).then(|| Task {
			id: run.task_id,
			workspace_id: run.workspace_id,
			title: "task".into(),
			description: String::new(),
			status: TaskStatus::Open,
			requirements: json!({}),
			owner: None,
			created_by: "root".into(),
			dependencies: vec![],
			parent_id: None,
			revision: 0,
			created_at: chrono::Utc::now(),
		}))
	}
	async fn task_visible(&mut self, _: &Task) -> Result<bool> {
		self.touch("task_visible")?;
		Ok(self.task_visible)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		let name = match action {
			"run.read" => "run.read",
			"memory.read" => "memory.read",
			_ => panic!("unexpected policy action"),
		};
		self.touch(name)?;
		self.decisions.push(resource.clone());
		Ok(self.denied != Some(name))
	}
	async fn human_reads(&mut self, _: Uuid, _: Uuid) -> Result<bool> {
		self.touch("humans")?;
		Ok(self.humans)
	}
	async fn run_reads(&mut self, _: Uuid) -> Result<bool> {
		self.touch("reads")?;
		Ok(self.reads)
	}
}

#[async_trait]
impl RunVisibilityScope for Scope {
	fn node_id(&self) -> &str {
		"aidash://node"
	}
	async fn scoped_admission(&mut self, _: Uuid) -> Result<bool> {
		self.touch("admission")?;
		Ok(self.admission)
	}
	async fn foreign_base_visible(&mut self, _: &RunMetadata) -> Result<bool> {
		self.touch("foreign")?;
		Ok(self.foreign)
	}
	async fn legacy_execution(&mut self, _: &RunMetadata) -> Result<bool> {
		self.touch("legacy")?;
		Ok(self.legacy)
	}
}

#[rstest]
#[tokio::test]
async fn local_disclosure_keeps_task_policy_memory_provenance_and_content_gates(
	run: RunMetadata,
	mut scope: Scope,
) {
	assert!(run_visible(&mut scope, &run).await.unwrap());
	assert_eq!(
		scope.calls,
		[
			"workspace",
			"memory",
			"task",
			"task_visible",
			"run.read",
			"memory.read",
			"humans",
			"reads"
		]
	);
	assert_eq!(scope.cache, Some(true));
	assert_eq!(scope.decisions[0].id, run.id.to_string());
	assert_eq!(
		scope.decisions[1].attributes,
		json!({"workspace_id":run.workspace_id,"owner":"root","created_by":qualified_agent(&run.home_node,&run.agent_id,&run.agent_version),"version":"1"})
	);
}

#[rstest]
#[case("missing_task")]
#[case("task_visible")]
#[case("run.read")]
#[case("memory.read")]
#[case("humans")]
#[case("reads")]
#[tokio::test]
async fn every_provenance_gate_can_hide_a_previously_admitted_run(
	run: RunMetadata,
	mut scope: Scope,
	#[case] gate: &'static str,
) {
	match gate {
		"missing_task" => scope.missing_task = true,
		"task_visible" => scope.task_visible = false,
		"humans" => scope.humans = false,
		"reads" => scope.reads = false,
		action => scope.denied = Some(action),
	}
	assert!(!run_visible(&mut scope, &run).await.unwrap());
	if gate != "reads" {
		assert!(!scope.calls.contains(&"reads"));
	}
	if matches!(gate, "missing_task" | "task_visible") {
		assert!(!scope.calls.contains(&"run.read"));
	}
	if gate == "run.read" {
		assert!(!scope.calls.contains(&"memory.read"));
	}
}

#[rstest]
#[case(true, false, vec!["humans"])]
#[case(false, true, vec![])]
#[tokio::test]
async fn cached_base_decisions_never_cache_live_human_membership(
	run: RunMetadata,
	mut scope: Scope,
	#[case] cached: bool,
	#[case] humans: bool,
	#[case] calls: Vec<&'static str>,
) {
	scope.cache = Some(cached);
	scope.humans = humans;
	assert!(!base_visible(&mut scope, &run).await.unwrap());
	assert_eq!(scope.calls, calls);
}

#[rstest]
#[tokio::test]
async fn dependency_frontier_prevents_reusing_or_publishing_partial_base_decisions(
	run: RunMetadata,
	mut scope: Scope,
) {
	scope.frontier = true;
	scope.cache = Some(false);
	assert!(base_visible(&mut scope, &run).await.unwrap());
	assert_eq!(scope.cache, Some(false));
	assert!(scope.calls.contains(&"task"));
}

#[rstest]
#[case(false, true)]
#[case(true, false)]
#[tokio::test]
async fn scoped_foreign_admission_cannot_fall_back_to_legacy_when_its_visibility_fails(
	mut run: RunMetadata,
	mut scope: Scope,
	#[case] foreign: bool,
	#[case] humans: bool,
) {
	run.home_node = "aidash://peer".into();
	scope.admission = true;
	scope.legacy = true;
	scope.foreign = foreign;
	scope.humans = humans;
	assert!(!run_visible(&mut scope, &run).await.unwrap());
	assert!(!scope.calls.contains(&"legacy"));
	assert!(!scope.calls.contains(&"task"));
	assert!(!scope.calls.contains(&"reads"));
}

#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn foreign_run_without_admission_requires_an_exact_legacy_grant(
	mut run: RunMetadata,
	mut scope: Scope,
	#[case] legacy: bool,
) {
	run.home_node = "aidash://peer".into();
	scope.legacy = legacy;
	scope.cache = Some(true);
	assert_eq!(run_visible(&mut scope, &run).await.unwrap(), legacy);
	assert_eq!(&scope.calls[..2], ["admission", "legacy"]);
	assert_eq!(scope.calls.contains(&"reads"), legacy);
}

#[rstest]
#[case("workspace")]
#[case("memory")]
#[case("task")]
#[case("task_visible")]
#[case("run.read")]
#[case("memory.read")]
#[case("humans")]
#[case("reads")]
#[case("admission")]
#[case("foreign")]
#[case("legacy")]
#[tokio::test]
async fn read_adapter_failures_are_preserved_and_stop_following_disclosure(
	run: RunMetadata,
	mut scope: Scope,
	#[case] boundary: &'static str,
) {
	let mut run = run;
	scope.fail = Some(boundary);
	if matches!(boundary, "admission" | "foreign" | "legacy") {
		run.home_node = "aidash://peer".into();
		scope.admission = boundary != "legacy";
	}
	let error = run_visible(&mut scope, &run).await.unwrap_err();
	assert!(matches!(error,Error::Port(ref error) if error.to_string()=="read adapter fault"));
	assert_eq!(scope.calls.last(), Some(&boundary));
}

struct Events {
	calls: Vec<&'static str>,
	resource: Option<bool>,
	present: bool,
	allowed: bool,
	human_allowed: bool,
	last_lookup: Option<(Uuid, Option<Uuid>)>,
	fail: Option<&'static str>,
}
impl Events {
	fn new() -> Self {
		Self {
			calls: vec![],
			resource: None,
			present: true,
			allowed: true,
			human_allowed: true,
			last_lookup: None,
			fail: None,
		}
	}
	fn touch(&mut self, name: &'static str) -> Result<()> {
		self.calls.push(name);
		if self.fail == Some(name) {
			return Err(Error::Port(Box::new(std::io::Error::other(
				"event adapter fault",
			))));
		}
		Ok(())
	}
	fn lookup(&mut self, name: &'static str, id: Uuid, workspace: Option<Uuid>) -> Result<()> {
		self.touch(name)?;
		self.last_lookup = Some((id, workspace));
		Ok(())
	}
}
fn event(kind: &str, data: Value) -> Event {
	Event {
		sequence: 1,
		id: Uuid::from_u128(8),
		node_id: "aidash://node".into(),
		workspace_id: Some(Uuid::from_u128(3)),
		kind: kind.into(),
		data,
		created_at: chrono::Utc::now(),
	}
}
#[async_trait]
impl EventVisibilityScope for Events {
	async fn marketplace_visible(&mut self, _: &Event) -> Result<bool> {
		self.touch("marketplace")?;
		Ok(self.allowed)
	}
	async fn resource_visible(&mut self, _: &Event) -> Result<Option<bool>> {
		self.touch("resource")?;
		Ok(self.resource)
	}
	async fn generation(
		&mut self,
		id: Uuid,
		workspace: Option<Uuid>,
	) -> Result<Option<aidash_domain::generation::requests::Request>> {
		self.lookup("generation", id, workspace)?;
		Ok(self.present.then(|| generation(1)))
	}
	async fn generation_visible(
		&mut self,
		_: &aidash_domain::generation::requests::Request,
	) -> Result<bool> {
		self.touch("generation_visible")?;
		Ok(self.allowed)
	}
	async fn conversation(
		&mut self,
		id: Uuid,
		workspace: Option<Uuid>,
	) -> Result<Option<Conversation>> {
		self.lookup("conversation", id, workspace)?;
		Ok(self.present.then(|| Conversation {
			id,
			workspace_id: workspace.unwrap(),
			created_by: "root".into(),
			target: "agent".into(),
			target_kind: "agent".into(),
			created_at: chrono::Utc::now(),
		}))
	}
	async fn conversation_resource(&mut self, c: &Conversation) -> Result<Resource> {
		self.touch("conversation_resource")?;
		Ok(Resource {
			tenant: "tenant".into(),
			kind: "conversation".into(),
			id: c.id.to_string(),
			attributes: json!({}),
		})
	}
	async fn decide(&mut self, _: &Resource, action: &str) -> Result<bool> {
		assert_eq!(action, "conversation.read");
		self.touch("conversation.read")?;
		Ok(self.allowed)
	}
	async fn human(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<Option<HumanRequest>> {
		self.lookup("human", id, workspace)?;
		Ok(self.present.then(|| HumanRequest {
			id,
			workspace_id: workspace.unwrap(),
			run_id: Uuid::from_u128(1),
			kind: "input".into(),
			prompt: String::new(),
			response: None,
			answered_by: None,
			created_at: chrono::Utc::now(),
		}))
	}
	async fn human_visible(&mut self, _: &HumanRequest) -> Result<bool> {
		self.touch("human_visible")?;
		Ok(self.human_allowed)
	}
	async fn run(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<Option<RunMetadata>> {
		self.lookup("run", id, workspace)?;
		Ok(self.present.then(run))
	}
	async fn run_visible(&mut self, _: &RunMetadata) -> Result<bool> {
		self.touch("run_visible")?;
		Ok(self.allowed)
	}
}

#[rstest]
#[case("marketplace.installed",Some(false),vec!["marketplace"],true)]
#[case("run.created",Some(false),vec!["resource"],false)]
#[case("run.created",Some(true),vec!["resource"],true)]
#[tokio::test]
async fn explicit_event_readers_take_precedence_over_generic_run_lookup(
	#[case] kind: &str,
	#[case] resource: Option<bool>,
	#[case] calls: Vec<&'static str>,
	#[case] allowed: bool,
) {
	let mut scope = Events::new();
	scope.resource = resource;
	assert_eq!(
		event_visible(&mut scope, &event(kind, json!({"run_id":"malformed"})))
			.await
			.unwrap(),
		allowed
	);
	assert_eq!(scope.calls, calls);
}

#[rstest]
#[case("workspace.created",json!({}),true)]
#[case("workspace.updated",json!({}),true)]
#[case("new.family",json!({}),false)]
#[case("run.updated",json!({"id":Uuid::from_u128(1)}),false)]
#[case("run.created",json!({"id":Uuid::from_u128(1),"run_id":null}),false)]
#[case("run.created",json!({"id":Uuid::from_u128(1),"run_id":"bad"}),false)]
#[case("generation.created",json!({"id":false}),false)]
#[case("conversation.created",json!({}),false)]
#[case("human.created",json!({"id":"bad"}),false)]
#[tokio::test]
async fn malformed_identifiers_and_unregistered_event_families_cannot_disclose_content(
	#[case] kind: &str,
	#[case] data: Value,
	#[case] allowed: bool,
) {
	let mut scope = Events::new();
	assert_eq!(
		event_visible(&mut scope, &event(kind, data)).await.unwrap(),
		allowed
	);
	assert_eq!(scope.calls, vec!["resource"]);
}

#[rstest]
#[case("run.created", "id")]
#[case("run.updated", "run_id")]
#[case("new.family", "run_id")]
#[tokio::test]
async fn generic_event_disclosure_uses_the_exact_workspace_and_run_reader(
	#[case] kind: &str,
	#[case] key: &str,
) {
	let mut scope = Events::new();
	let id = Uuid::from_u128(1);
	let mut data = json!({});
	data[key] = json!(id);
	assert!(event_visible(&mut scope, &event(kind, data)).await.unwrap());
	assert_eq!(scope.last_lookup, Some((id, Some(Uuid::from_u128(3)))));
	assert_eq!(scope.calls, vec!["resource", "run", "run_visible"]);
}

#[rstest]
#[case("generation.created", true, true)]
#[case("generation.created", false, true)]
#[case("generation.created", true, false)]
#[case("conversation.created", true, true)]
#[case("conversation.created", false, true)]
#[case("conversation.created", true, false)]
#[tokio::test]
async fn dedicated_event_families_require_the_current_record_and_read_policy(
	#[case] kind: &str,
	#[case] present: bool,
	#[case] allowed: bool,
) {
	let mut scope = Events::new();
	scope.present = present;
	scope.allowed = allowed;
	assert_eq!(
		event_visible(&mut scope, &event(kind, json!({"id":Uuid::from_u128(1)})))
			.await
			.unwrap(),
		present && allowed
	);
	assert!(!scope.calls.contains(&"run"));
}

#[rstest]
#[case(false, true)]
#[case(true, false)]
#[case(true, true)]
#[tokio::test]
async fn human_events_require_both_human_permission_and_complete_run_provenance(
	#[case] human: bool,
	#[case] run_allowed: bool,
) {
	let mut scope = Events::new();
	scope.human_allowed = human;
	scope.allowed = run_allowed;
	assert_eq!(
		event_visible(
			&mut scope,
			&event(
				"human.answered",
				json!({"id":Uuid::from_u128(5),"run_id":Uuid::from_u128(1)})
			)
		)
		.await
		.unwrap(),
		human && run_allowed
	);
	assert_eq!(scope.calls.contains(&"run_visible"), human);
}

#[rstest]
#[tokio::test]
async fn a_visible_human_record_without_a_run_reference_still_does_not_disclose_an_event() {
	let mut scope = Events::new();
	assert!(
		!event_visible(
			&mut scope,
			&event("human.answered", json!({"id":Uuid::from_u128(5)}))
		)
		.await
		.unwrap()
	);
	assert_eq!(scope.calls, vec!["resource", "human", "human_visible"]);
}

#[rstest]
#[case("resource", "run.created")]
#[case("marketplace", "marketplace.installed")]
#[case("generation", "generation.created")]
#[case("generation_visible", "generation.created")]
#[case("conversation", "conversation.created")]
#[case("conversation_resource", "conversation.created")]
#[case("conversation.read", "conversation.created")]
#[case("human", "human.created")]
#[case("human_visible", "human.created")]
#[case("run", "run.created")]
#[case("run_visible", "run.created")]
#[tokio::test]
async fn event_read_failures_preserve_their_error_and_stop_following_lookups(
	#[case] boundary: &'static str,
	#[case] kind: &str,
) {
	let mut scope = Events::new();
	scope.fail = Some(boundary);
	let error = event_visible(
		&mut scope,
		&event(
			kind,
			json!({"id":Uuid::from_u128(1),"run_id":Uuid::from_u128(1)}),
		),
	)
	.await
	.unwrap_err();
	assert!(matches!(error,Error::Port(ref error) if error.to_string()=="event adapter fault"));
	assert_eq!(scope.calls.last(), Some(&boundary));
}

fn generation(id: u128) -> aidash_domain::generation::requests::Request {
	let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp(1000, 0).unwrap();
	aidash_domain::generation::requests::Request {
		id: Uuid::from_u128(id),
		tenant: "tenant".into(),
		policy_id: "policy".into(),
		policy_revision: 7,
		task_id: Uuid::from_u128(2),
		home_node: String::new(),
		foreign_intent: None,
		prepared: true,
		grant_id: None,
		admission_id: None,
		workspace_id: Uuid::from_u128(3),
		credential_id: Uuid::from_u128(4),
		root_subject: "alice".into(),
		subject_chain: vec!["alice".into()],
		agent_id: "agent".into(),
		agent_version: "1.0.0".into(),
		definition: json!({}),
		status: "ACTIVE".into(),
		reason: "work".into(),
		depth: 1,
		token_limit: 10000,
		quota_released: false,
		expires_at: timestamp,
		created_at: timestamp,
	}
}
