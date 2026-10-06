use super::*;
use aidash_domain::{
	RunControl, RunPhase, Task, TaskStatus,
	policy::{PolicyBundle, Resource, SubjectKind},
	registry::Entry,
};
use async_trait::async_trait;
use chrono::Utc;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use uuid::Uuid;
fn id(value: u128) -> Uuid {
	Uuid::from_u128(value)
}
fn reference(id: &str) -> EntityRef {
	EntityRef {
		id: id.into(),
		version: "1".into(),
	}
}
#[fixture]
fn run() -> RunMetadata {
	RunMetadata {
		id: id(1),
		task_id: id(2),
		workspace_id: id(3),
		home_node: "aidash://home".into(),
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
#[fixture]
fn agent() -> AgentConfig {
	serde_json::from_value(json!({"model":reference("model"),"tools":[reference("tool")],"skills":[reference("skill")],"cluster":reference("configured-cluster"),"core_capabilities":{"files":true},"allow_task_creation":false})).unwrap()
}
struct Scope {
	visible: bool,
	clusters: Vec<String>,
	bundle: PolicyBundle,
	entry: Entry,
	calls: Vec<String>,
	decisions: Vec<(Resource, String)>,
	fail: Option<String>,
	deny: Option<String>,
}
#[fixture]
fn scope(agent: AgentConfig) -> Scope {
	let subject = qualified_agent("aidash://local", "producer", "1");
	Scope {
		visible: true,
		clusters: vec!["conversation-cluster@1".into()],
		bundle: serde_json::from_value(
			json!({"tenant":"tenant","subjects":{subject:{"kind":"agent","delegated_by":null}}}),
		)
		.unwrap(),
		entry: serde_json::from_value(
			json!({"id":"producer","version":"1","kind":"agent","name":{},"description":{},"config":agent}),
		)
		.unwrap(),
		calls: vec![],
		decisions: vec![],
		fail: None,
		deny: None,
	}
}
impl Scope {
	fn record(&mut self, name: impl Into<String>) -> Result<()> {
		let name = name.into();
		self.calls.push(name.clone());
		if self.fail.as_ref() == Some(&name) {
			return Err(Error::Port(Box::new(std::io::Error::other(format!(
				"{name} fault"
			)))));
		}
		if self.deny.as_ref() == Some(&name) {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
}
#[async_trait]
impl RunGuardScope for Scope {
	fn node_id(&self) -> &str {
		"aidash://local"
	}
	fn bundle(&self) -> &PolicyBundle {
		&self.bundle
	}
	async fn run_visible(&mut self, run: &RunMetadata) -> Result<bool> {
		assert_eq!(run.id, id(1));
		self.record("visible")?;
		Ok(self.visible)
	}
	async fn cluster_targets(&mut self, workspace: Uuid) -> Result<Vec<String>> {
		assert_eq!(workspace, id(3));
		self.record("clusters")?;
		Ok(self.clusters.clone())
	}
	async fn catalog(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		self.record(format!(
			"catalog:{}@{}:{action}",
			reference.id, reference.version
		))?;
		Ok(self.entry.clone())
	}
	async fn task_read(&mut self, task: Uuid) -> Result<Task> {
		assert_eq!(task, id(2));
		self.record("task_read")?;
		Ok(Task {
			id: task,
			workspace_id: id(3),
			title: "saved task".into(),
			description: String::new(),
			status: TaskStatus::Open,
			requirements: json!({}),
			owner: None,
			created_by: "creator".into(),
			dependencies: vec![],
			parent_id: None,
			revision: 7,
			created_at: Utc::now(),
		})
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.record("task_resource")?;
		Ok(Resource {
			tenant: "tenant".into(),
			kind: "task".into(),
			id: task.id.to_string(),
			attributes: json!({"creator":task.created_by,"revision":task.revision}),
		})
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.decisions.push((resource.clone(), action.into()));
		self.record(action)
	}
	async fn require_live(&mut self, task: Uuid, agent: &EntityRef) -> Result<()> {
		assert_eq!(task, id(2));
		assert_eq!(agent, &reference("producer"));
		self.record("live")
	}
	async fn check_pinned(&mut self, entry: &Entry) -> Result<()> {
		assert_eq!(entry.id, "producer");
		self.record("pinned")
	}
	async fn context_authority(&mut self, run: &RunMetadata) -> Result<()> {
		assert_eq!(run.workspace_id, id(3));
		self.record("context")
	}
}
fn expected() -> Vec<&'static str> {
	vec![
		"visible",
		"clusters",
		"catalog:conversation-cluster@1:cluster.execute",
		"task_read",
		"task_resource",
		"task.execute",
		"live",
		"catalog:producer@1:agent.execute",
		"pinned",
		"context",
		"catalog:model@1:registry.read",
		"catalog:tool@1:registry.read",
		"catalog:skill@1:registry.read",
		"catalog:configured-cluster@1:registry.read",
	]
}
#[rstest]
#[tokio::test]
async fn guard_rechecks_execution_and_every_dependency_in_original_order(
	mut scope: Scope,
	run: RunMetadata,
	agent: AgentConfig,
) {
	let current = authorize(&mut scope, &run, true).await.unwrap();
	assert_eq!(scope.calls, expected());
	assert_eq!(
		serde_json::to_value(current).unwrap(),
		serde_json::to_value(agent).unwrap()
	);
	assert_eq!(scope.decisions.len(), 1);
	assert_eq!(scope.decisions[0].1, "task.execute");
	assert_eq!(
		scope.decisions[0].0.attributes,
		json!({"creator":"creator","revision":7})
	);
}
#[rstest]
#[case::without_context_read(false, true)]
#[case::without_core_capabilities(true, false)]
#[case::both_absent(false, false)]
#[tokio::test]
async fn guard_skips_context_only_when_unrequested_or_all_core_capabilities_are_disabled(
	mut scope: Scope,
	run: RunMetadata,
	mut agent: AgentConfig,
	#[case] read_context: bool,
	#[case] capabilities: bool,
) {
	agent.core_capabilities.files = capabilities;
	scope.entry.config = serde_json::to_value(agent).unwrap();
	authorize(&mut scope, &run, read_context).await.unwrap();
	let mut expected = expected();
	expected.retain(|name| *name != "context");
	assert_eq!(scope.calls, expected);
}
#[rstest]
#[tokio::test]
async fn invisible_run_is_forbidden_before_conversations_or_catalog(
	mut scope: Scope,
	run: RunMetadata,
) {
	scope.visible = false;
	assert!(matches!(
		authorize(&mut scope, &run, true).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["visible"]);
}
#[rstest]
#[tokio::test]
async fn a_conversation_cluster_remains_required_without_a_configured_agent_cluster(
	mut scope: Scope,
	run: RunMetadata,
	mut agent: AgentConfig,
) {
	agent.cluster = None;
	scope.entry.config = serde_json::to_value(agent).unwrap();
	authorize(&mut scope, &run, true).await.unwrap();
	assert!(
		scope
			.calls
			.contains(&"catalog:conversation-cluster@1:cluster.execute".into())
	);
	assert!(
		!scope
			.calls
			.contains(&"catalog:configured-cluster@1:registry.read".into())
	);
}
#[rstest]
#[tokio::test]
async fn cluster_targets_preserve_duplicate_order_and_split_at_the_last_separator(
	mut scope: Scope,
	run: RunMetadata,
) {
	scope.clusters = vec!["org@cluster@2".into(), "same@1".into(), "same@1".into()];
	authorize(&mut scope, &run, true).await.unwrap();
	assert_eq!(
		scope.calls[2..5],
		vec![
			"catalog:org@cluster@2:cluster.execute",
			"catalog:same@1:cluster.execute",
			"catalog:same@1:cluster.execute"
		]
	);
}
#[rstest]
#[case::first(false)]
#[case::after_valid(true)]
#[tokio::test]
async fn malformed_cluster_targets_are_forbidden_before_task_execution(
	mut scope: Scope,
	run: RunMetadata,
	#[case] after_valid: bool,
) {
	scope.clusters = if after_valid {
		vec!["valid@1".into(), "malformed".into()]
	} else {
		vec!["malformed".into()]
	};
	assert!(matches!(
		authorize(&mut scope, &run, true).await,
		Err(Error::Forbidden)
	));
	let mut expected = vec!["visible", "clusters"];
	if after_valid {
		expected.push("catalog:valid@1:cluster.execute");
	}
	assert_eq!(scope.calls, expected);
}
#[rstest]
#[case::missing(true)]
#[case::wrong_kind(false)]
#[tokio::test]
async fn a_guard_requires_the_agent_subject_on_the_current_node_before_pinned_installation(
	mut scope: Scope,
	run: RunMetadata,
	#[case] missing: bool,
) {
	let subject = qualified_agent("aidash://local", "producer", "1");
	if missing {
		scope.bundle.subjects.remove(&subject);
	} else {
		scope.bundle.subjects.get_mut(&subject).unwrap().kind = SubjectKind::User;
	}
	assert!(matches!(
		authorize(&mut scope, &run, true).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, expected()[..8]);
	assert!(!scope.calls.contains(&"pinned".into()));
}
#[rstest]
#[tokio::test]
async fn invalid_agent_configuration_keeps_the_json_error_before_context_or_registry_reads(
	mut scope: Scope,
	run: RunMetadata,
) {
	scope.entry.config = Value::Null;
	assert!(matches!(
		authorize(&mut scope, &run, true).await,
		Err(Error::Json(_))
	));
	assert_eq!(scope.calls, expected()[..9]);
}
#[rstest]
#[case::visibility("visible")]
#[case::conversation("clusters")]
#[case::cluster("catalog:conversation-cluster@1:cluster.execute")]
#[case::task("task_read")]
#[case::resource("task_resource")]
#[case::execute("task.execute")]
#[case::live_generation("live")]
#[case::agent("catalog:producer@1:agent.execute")]
#[case::installation("pinned")]
#[case::context("context")]
#[case::model("catalog:model@1:registry.read")]
#[case::tool("catalog:tool@1:registry.read")]
#[case::skill("catalog:skill@1:registry.read")]
#[case::configured_cluster("catalog:configured-cluster@1:registry.read")]
#[tokio::test]
async fn any_guard_adapter_fault_stops_at_that_boundary_and_preserves_its_identity(
	mut scope: Scope,
	run: RunMetadata,
	#[case] stage: &str,
) {
	scope.fail = Some(stage.into());
	let Error::Port(error) = authorize(&mut scope, &run, true).await.err().unwrap() else {
		panic!("expected guard fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		format!("{stage} fault")
	);
	let expected = expected();
	let last = expected.iter().position(|name| *name == stage).unwrap();
	assert_eq!(scope.calls, expected[..=last]);
}
#[rstest]
#[case::conversation_cluster("catalog:conversation-cluster@1:cluster.execute")]
#[case::task("task.execute")]
#[case::model("catalog:model@1:registry.read")]
#[case::tool("catalog:tool@1:registry.read")]
#[case::skill("catalog:skill@1:registry.read")]
#[case::cluster("catalog:configured-cluster@1:registry.read")]
#[tokio::test]
async fn revoked_current_dependency_approval_prevents_accepting_agent_output(
	mut scope: Scope,
	run: RunMetadata,
	#[case] stage: &str,
) {
	scope.deny = Some(stage.into());
	assert!(matches!(
		authorize(&mut scope, &run, true).await,
		Err(Error::Forbidden)
	));
	let expected = expected();
	let last = expected.iter().position(|name| *name == stage).unwrap();
	assert_eq!(scope.calls, expected[..=last]);
}
#[rstest]
#[tokio::test]
async fn repeated_configured_references_are_rechecked_without_deduplication(
	mut scope: Scope,
	run: RunMetadata,
	mut agent: AgentConfig,
) {
	agent.tools = vec![agent.model.clone(), agent.model.clone()];
	agent.skills = vec![agent.model.clone()];
	agent.cluster = None;
	scope.entry.config = serde_json::to_value(agent).unwrap();
	authorize(&mut scope, &run, false).await.unwrap();
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|name| name.as_str() == "catalog:model@1:registry.read")
			.count(),
		4
	);
	assert!(!scope.calls.contains(&"context".into()));
}
