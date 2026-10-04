use super::*;
use crate::ports::authorization::visibility::{
	provenance::RecordedRead, resources::ResourceVisibilityScope,
};
use aidash_domain::{
	Artifact, Conversation, HumanRequest, Message, RunMetadata, Task,
	generation::requests::Request, policy::Resource, registry::EntityRef,
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use std::collections::BTreeMap;
struct Scope {
	graph: BTreeMap<Uuid, Vec<RecordedRead>>,
	entries: Vec<EntityRef>,
	calls: Vec<String>,
	decisions: Vec<Resource>,
	missing: bool,
	denied: Option<&'static str>,
	fail: Option<&'static str>,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		graph: BTreeMap::new(),
		entries: vec![],
		calls: vec![],
		decisions: vec![],
		missing: false,
		denied: None,
		fail: None,
	}
}
impl Scope {
	fn touch(&mut self, name: &'static str) -> Result<()> {
		self.calls.push(name.into());
		if self.fail == Some(name) {
			return Err(Error::Port(Box::new(std::io::Error::other(
				"provenance adapter fault",
			))));
		}
		Ok(())
	}
	fn lookup(&mut self, name: &'static str, id: Uuid, workspace: Uuid) -> Result<()> {
		self.touch(name)?;
		self.calls.push(format!("lookup:{name}:{id}:{workspace}"));
		Ok(())
	}
}
#[async_trait]
impl ResourceVisibilityScope for Scope {
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
		Ok(self.resource("workspace", &id.to_string(), json!({"workspace_id":id})))
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		let key = match action {
			"workspace.events" => "workspace.events",
			"task.read" => "task.read",
			"artifact.read" => "artifact.read",
			"message.read" => "message.read",
			"conversation.read" => "conversation.read",
			_ => panic!("unexpected action"),
		};
		self.touch(key)?;
		self.decisions.push(resource.clone());
		Ok(self.denied != Some(key))
	}
	async fn artifact_task(&mut self, a: &Artifact) -> Result<Option<Task>> {
		self.lookup("artifact_task", a.task_id, a.workspace_id)?;
		let mut t = task();
		t.id = a.task_id;
		t.workspace_id = a.workspace_id;
		Ok(Some(t))
	}
	async fn output_visible(&mut self, _: Uuid, _: &str, _: Uuid) -> Result<bool> {
		self.touch("output")?;
		Ok(self.denied != Some("output"))
	}
	fn cached_human(&self, _: Uuid) -> Option<bool> {
		panic!("journal traversal does not directly cache human requests")
	}
	fn remember_human(&mut self, _: Uuid, _: bool) {
		panic!("journal traversal does not directly cache human requests")
	}
	async fn humans(&mut self, _: Uuid, _: Uuid) -> Result<Vec<HumanRequest>> {
		panic!("human membership is checked by the base Run reader")
	}
}
#[async_trait]
impl ReadProvenanceScope for Scope {
	async fn registry_entries(&mut self, run: Uuid) -> Result<Vec<EntityRef>> {
		self.touch("registry")?;
		self.calls.push(format!("visited:{run}"));
		Ok(self.entries.clone())
	}
	async fn catalog_read(&mut self, reference: &EntityRef) -> Result<()> {
		self.touch("catalog")?;
		self.calls
			.push(format!("catalog:{}:{}", reference.id, reference.version));
		if self.denied == Some("catalog") {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn remote_reads(&mut self, _: Uuid) -> Result<bool> {
		self.touch("remote")?;
		Ok(self.denied != Some("remote"))
	}
	async fn semantic_reads(&mut self, _: Uuid) -> Result<bool> {
		self.touch("semantic")?;
		Ok(self.denied != Some("semantic"))
	}
	async fn received_semantic(&mut self, _: Uuid) -> Result<bool> {
		self.touch("received")?;
		Ok(self.denied != Some("received"))
	}
	async fn sources(&mut self, run: Uuid) -> Result<Vec<RecordedRead>> {
		self.touch("sources")?;
		Ok(self.graph.get(&run).cloned().unwrap_or_default())
	}
	async fn source_task(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Task>> {
		self.lookup("task", id, workspace)?;
		let mut t = task();
		t.id = id;
		t.workspace_id = workspace;
		Ok((!self.missing).then_some(t))
	}
	async fn source_artifact(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Artifact>> {
		self.lookup("artifact", id, workspace)?;
		let mut a = artifact();
		a.id = id;
		a.workspace_id = workspace;
		Ok((!self.missing).then_some(a))
	}
	async fn source_message(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Message>> {
		self.lookup("message", id, workspace)?;
		let mut m = message(id.as_u128(), "stored-sender");
		m.workspace_id = workspace;
		Ok((!self.missing).then_some(m))
	}
	async fn source_run(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<RunMetadata>> {
		self.lookup("run", id, workspace)?;
		let mut r = run();
		r.id = id;
		r.workspace_id = workspace;
		Ok((!self.missing).then_some(r))
	}
	async fn source_conversation(
		&mut self,
		id: Uuid,
		workspace: Uuid,
	) -> Result<Option<Conversation>> {
		self.lookup("conversation", id, workspace)?;
		Ok((!self.missing).then(|| Conversation {
			id,
			workspace_id: workspace,
			created_by: "stored-author".into(),
			target: "agent".into(),
			target_kind: "agent".into(),
			created_at: chrono::Utc::now(),
		}))
	}
	async fn source_generation(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Request>> {
		self.lookup("generation", id, workspace)?;
		let mut job = generation(id.as_u128());
		job.workspace_id = workspace;
		Ok((!self.missing).then_some(job))
	}
	async fn run_base_visible(&mut self, _: &RunMetadata) -> Result<bool> {
		self.touch("base")?;
		Ok(self.denied != Some("base"))
	}
	async fn generation_visible(&mut self, _: &Request) -> Result<bool> {
		self.touch("generation.read")?;
		Ok(self.denied != Some("generation.read"))
	}
}
#[rstest]
#[tokio::test]
async fn cycles_and_duplicate_run_references_terminate_after_checking_each_journal(
	mut scope: Scope,
) {
	let a = Uuid::from_u128(1);
	let b = Uuid::from_u128(2);
	let w = Uuid::from_u128(3);
	scope
		.graph
		.insert(a, vec![(w, "run".into(), b), (w, "run".into(), b)]);
	scope.graph.insert(
		b,
		vec![(w, "run".into(), a), (w, "task".into(), Uuid::from_u128(4))],
	);
	assert!(run_reads_visible(&mut scope, a).await.unwrap());
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|c| c.starts_with("visited:"))
			.cloned()
			.collect::<Vec<_>>(),
		vec![format!("visited:{a}"), format!("visited:{b}")]
	);
	assert!(scope.calls.contains(&"task.read".into()));
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|c| c.as_str() == "sources")
			.count(),
		2
	);
}
#[rstest]
#[tokio::test]
async fn long_run_dependency_chains_use_a_worklist_instead_of_recursive_stack_frames(
	mut scope: Scope,
) {
	let workspace = Uuid::from_u128(9000);
	for id in 1..512 {
		scope.graph.insert(
			Uuid::from_u128(id),
			vec![(workspace, "run".into(), Uuid::from_u128(id + 1))],
		);
	}
	assert!(
		run_reads_visible(&mut scope, Uuid::from_u128(1))
			.await
			.unwrap()
	);
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|c| c.starts_with("visited:"))
			.count(),
		512
	);
}
#[rstest]
#[case("catalog")]
#[case("remote")]
#[case("semantic")]
#[case("received")]
#[tokio::test]
async fn hidden_dependency_journals_stop_before_loading_any_source_records(
	mut scope: Scope,
	#[case] gate: &'static str,
) {
	scope.denied = Some(gate);
	scope.entries = vec![EntityRef {
		id: "entry".into(),
		version: "1".into(),
	}];
	assert!(
		!run_reads_visible(&mut scope, Uuid::from_u128(1))
			.await
			.unwrap()
	);
	assert!(!scope.calls.contains(&"sources".into()));
}
#[rstest]
#[tokio::test]
async fn denied_nested_run_stops_before_reading_its_journal_or_later_sources(mut scope: Scope) {
	let root = Uuid::from_u128(1);
	scope.denied = Some("base");
	scope.graph.insert(
		root,
		vec![
			(Uuid::from_u128(3), "run".into(), Uuid::from_u128(2)),
			(Uuid::from_u128(3), "task".into(), Uuid::from_u128(5)),
		],
	);
	assert!(!run_reads_visible(&mut scope, root).await.unwrap());
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|c| c.starts_with("visited:"))
			.count(),
		1
	);
	assert!(!scope.calls.contains(&"task".into()));
}
#[rstest]
#[case("task")]
#[case("artifact")]
#[case("message")]
#[case("run")]
#[case("conversation")]
#[case("generation")]
#[tokio::test]
async fn source_lookups_retain_the_exact_journal_workspace_and_missing_rows_fail_closed(
	mut scope: Scope,
	#[case] kind: &str,
) {
	let workspace = Uuid::from_u128(3);
	let id = Uuid::from_u128(6);
	let mut pending = vec![];
	scope.missing = true;
	assert!(
		!source_visible(&mut scope, workspace, kind, id, &mut pending)
			.await
			.unwrap()
	);
	assert!(pending.is_empty());
	assert_eq!(
		scope.calls,
		vec![kind.to_owned(), format!("lookup:{kind}:{id}:{workspace}")]
	);
}
#[rstest]
#[case("workspace_events", "workspace.events")]
#[case("task", "task.read")]
#[case("artifact", "artifact.read")]
#[case("message", "message.read")]
#[case("run", "base")]
#[case("conversation", "conversation.read")]
#[case("generation", "generation.read")]
#[tokio::test]
async fn every_source_family_requires_its_current_read_policy(
	mut scope: Scope,
	#[case] kind: &str,
	#[case] gate: &'static str,
) {
	scope.denied = Some(gate);
	let mut pending = vec![];
	assert!(
		!source_visible(
			&mut scope,
			Uuid::from_u128(3),
			kind,
			Uuid::from_u128(6),
			&mut pending
		)
		.await
		.unwrap()
	);
	assert_eq!(
		pending,
		if kind == "run" {
			vec![Uuid::from_u128(6)]
		} else {
			vec![]
		}
	);
}
#[rstest]
#[case("unknown")]
#[case("")]
#[case("Task")]
#[tokio::test]
async fn unregistered_journal_source_kinds_fail_closed_without_queries(
	mut scope: Scope,
	#[case] kind: &str,
) {
	let mut pending = vec![];
	assert!(
		!source_visible(
			&mut scope,
			Uuid::from_u128(3),
			kind,
			Uuid::from_u128(6),
			&mut pending
		)
		.await
		.unwrap()
	);
	assert!(scope.calls.is_empty());
	assert!(pending.is_empty());
}
#[rstest]
#[tokio::test]
async fn successful_nested_run_is_enqueued_for_complete_provenance_checks(mut scope: Scope) {
	let mut pending = vec![Uuid::from_u128(99)];
	assert!(
		source_visible(
			&mut scope,
			Uuid::from_u128(3),
			"run",
			Uuid::from_u128(6),
			&mut pending
		)
		.await
		.unwrap()
	);
	assert_eq!(pending, vec![Uuid::from_u128(99), Uuid::from_u128(6)]);
}
#[rstest]
#[case("registry")]
#[case("catalog")]
#[case("remote")]
#[case("semantic")]
#[case("received")]
#[case("sources")]
#[tokio::test]
async fn journal_and_catalog_failures_are_preserved_without_becoming_denial_or_success(
	mut scope: Scope,
	#[case] boundary: &'static str,
) {
	scope.fail = Some(boundary);
	scope.entries = vec![EntityRef {
		id: "entry".into(),
		version: "1".into(),
	}];
	let error = run_reads_visible(&mut scope, Uuid::from_u128(1))
		.await
		.unwrap_err();
	assert!(matches!(error,Error::Port(ref e) if e.to_string()=="provenance adapter fault"));
	assert_eq!(scope.calls.last().map(String::as_str), Some(boundary));
}
#[rstest]
#[case("task")]
#[case("artifact")]
#[case("message")]
#[case("run")]
#[case("conversation")]
#[case("generation")]
#[case("base")]
#[case("generation.read")]
#[tokio::test]
async fn source_read_errors_stop_following_provenance_checks(
	mut scope: Scope,
	#[case] boundary: &'static str,
) {
	scope.fail = Some(boundary);
	let kind = match boundary {
		"base" => "run",
		"generation.read" => "generation",
		kind => kind,
	};
	let mut pending = vec![];
	let error = source_visible(
		&mut scope,
		Uuid::from_u128(3),
		kind,
		Uuid::from_u128(6),
		&mut pending,
	)
	.await
	.unwrap_err();
	assert!(matches!(error,Error::Port(ref e) if e.to_string()=="provenance adapter fault"));
	assert_eq!(scope.calls.last().map(String::as_str), Some(boundary));
}

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

fn generation(id: u128) -> Request {
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

#[fixture]
fn task() -> Task {
	Task {
		id: Uuid::from_u128(1),
		workspace_id: Uuid::from_u128(2),
		title: "Task".into(),
		description: String::new(),
		status: aidash_domain::TaskStatus::Open,
		requirements: json!({}),
		owner: None,
		created_by: "stored-task-author".into(),
		dependencies: vec![],
		parent_id: None,
		revision: 0,
		created_at: chrono::Utc::now(),
	}
}

#[fixture]
fn artifact() -> Artifact {
	Artifact {
		id: Uuid::from_u128(3),
		workspace_id: Uuid::from_u128(2),
		task_id: Uuid::from_u128(1),
		kind: "text".into(),
		name: "Artifact".into(),
		content: json!({}),
		created_by: "stored-artifact-author".into(),
		idempotency_key: "key".into(),
		created_at: chrono::Utc::now(),
	}
}

fn message(id: u128, sender: &str) -> Message {
	Message {
		id: Uuid::from_u128(id),
		workspace_id: Uuid::from_u128(2),
		sender: sender.into(),
		content: "same content".into(),
		idempotency_key: None,
		created_at: chrono::Utc::now(),
	}
}
