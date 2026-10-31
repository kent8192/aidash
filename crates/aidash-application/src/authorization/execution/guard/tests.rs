//! Current authority is rechecked over the complete admitted closure.
use super::*;
use aidash_domain::{
	RunControl, RunPhase, Task, TaskStatus,
	policy::{PolicyBundle, Resource},
	registry::{Entry, bindings::BindingSnapshot},
};
use async_trait::async_trait;
use chrono::Utc;
use rstest::{fixture, rstest};
use serde_json::json;
use uuid::Uuid;
#[fixture]
fn run() -> RunMetadata {
	RunMetadata {
		node: "brave-otter-k3m9q2xa".parse().unwrap(),
		id: Uuid::from_u128(1),
		task_id: Uuid::from_u128(2),
		workspace_id: Uuid::from_u128(3),
		home_node: "aidash://local/brave-otter-k3m9q2xa".into(),
		agent_id: "producer".into(),
		agent_version: "1.0.0".into(),
		phase: RunPhase::Ready,
		control: RunControl::Active,
		step: 0,
		revision: 5,
		observed_input_seq: 0,
		ledger_worker_ready: false,
		control_revision: 0,
		control_source: Default::default(),
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: Utc::now(),
	}
}
struct Scope {
	visible: bool,
	clusters: Vec<String>,
	bundle: PolicyBundle,
	snapshot: BindingSnapshot,
	calls: Vec<String>,
	fail: Option<String>,
	deny: Option<String>,
	changed: Option<String>,
}
#[fixture]
fn scope() -> Scope {
	let node = "aidash://local";
	let subject =
		aidash_domain::agent_subject("aidash://local/brave-otter-k3m9q2xa", "producer", "1.0.0");
	let mut agent = crate::test_support::agent("producer");
	agent.config["remove_default"] = json!(aidash_domain::registry::bindings::DEFAULT_TOOLS);
	agent.config["bindings"] = json!([
		{"kind":"memory","target":{"registry_deployment":node,"id":"memory","version":"1.0.0"}},
		{"kind":"decider","target":{"registry_deployment":node,"id":"private.compaction","version":"1.0.0"}},
		{"kind":"tool","target":{"registry_deployment":node,"id":"tool","version":"1.0.0"}}
	]);
	let memory = crate::test_support::entry(
		"memory",
		"memory",
		json!({"schema_version":1,"source":{"adapter":"conversation_memory"}}),
	);
	let tool = crate::test_support::entry(
		"tool",
		"tool",
		json!({"registry_deployment":node,"provider":"integration.http@1","operation":"invoke","default_alias":"lookup","tier":"integration","transport":{"transport":"http","endpoint":"https://fixture.invalid","replay":"unsafe"}}),
	);
	let mut compaction = crate::registry::system::compaction_decider().unwrap();
	compaction.id = "private.compaction".into();
	Scope {
		visible: true,
		clusters: vec!["conversation-cluster@1.0.0".into()],
		bundle: serde_json::from_value(
			json!({"tenant":"tenant","subjects":{subject:{"kind":"agent", "node": aidash_domain::nodes::NodeName::derived("acme"),"delegated_by":null}}}),
		)
		.unwrap(),
		snapshot: crate::test_support::resolve(node, &agent, false, vec![memory, tool, compaction]),
		calls: vec![],
		fail: None,
		deny: None,
		changed: None,
	}
}
impl Scope {
	fn record(&mut self, name: impl Into<String>) -> Result<()> {
		let name = name.into();
		self.calls.push(name.clone());
		if self.fail.as_ref() == Some(&name) {
			return Err(Error::Port(Box::new(std::io::Error::other(name))));
		}
		if self.deny.as_ref() == Some(&name) {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	fn current(&self, reference: &EntityRef) -> Entry {
		let mut entry = self
			.snapshot
			.definitions
			.iter()
			.find(|d| {
				d.identity.registry_deployment == self.node_id() && d.identity.local() == *reference
			})
			.map(|d| d.definition.clone())
			.unwrap_or_else(|| crate::test_support::entry(&reference.id, "cluster", json!({})));
		if self.changed.as_deref() == Some(&reference.id) {
			entry.tags.push("changed".into());
		}
		entry
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
	async fn run_visible(&mut self, _: &RunMetadata) -> Result<bool> {
		self.record("visible")?;
		Ok(self.visible)
	}
	async fn cluster_targets(&mut self, _: Uuid) -> Result<Vec<String>> {
		self.record("clusters")?;
		Ok(self.clusters.clone())
	}
	async fn catalog(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		self.record(format!("catalog:{}:{action}", reference.id))?;
		Ok(self.current(reference))
	}
	async fn closure_catalog(
		&mut self,
		reference: &EntityRef,
		action: &str,
		users: &[EntityRef],
	) -> Result<Entry> {
		let users = users
			.iter()
			.map(|user| user.id.as_str())
			.collect::<Vec<_>>()
			.join(",");
		self.record(format!("closure:{}:{action}:{users}", reference.id))?;
		Ok(self.current(reference))
	}
	async fn metadata(&mut self, reference: &EntityRef) -> Result<Entry> {
		self.record(format!("metadata:{}:registry.read", reference.id))?;
		Ok(self.current(reference))
	}
	async fn task_read(&mut self, id: Uuid) -> Result<Task> {
		self.record("task_read")?;
		Ok(Task {
			id,
			workspace_id: Uuid::from_u128(3),
			title: "Saved task".into(),
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
	async fn require(&mut self, _: &Resource, action: &str) -> Result<()> {
		self.record(action)
	}
	async fn require_live(&mut self, _: Uuid, _: &EntityRef) -> Result<()> {
		self.record("live")
	}
	async fn check_pinned(&mut self, _: &Entry) -> Result<()> {
		self.record("pinned")
	}
	async fn context_authority(&mut self, _: &RunMetadata) -> Result<()> {
		self.record("context")
	}
	async fn binding_snapshot(&mut self, _: &RunMetadata) -> Result<BindingSnapshot> {
		self.record("snapshot")?;
		Ok(self.snapshot.clone())
	}
}
#[rstest]
#[tokio::test]
async fn opt_in_memory_keeps_current_source_authority_without_a_native_area(
	mut scope: Scope,
	run: RunMetadata,
) {
	let config = authorize(&mut scope, &run, true).await.unwrap();
	assert!(config.conversation_memory);
	assert!(!config.core_capabilities.enabled());
	assert!(!scope.calls.contains(&"context".into()));
	for definition in &scope.snapshot.definitions {
		let path = if definition.identity.id == "private.compaction" {
			"metadata"
		} else {
			"catalog"
		};
		assert!(
			scope
				.calls
				.contains(&format!("{path}:{}:registry.read", definition.identity.id))
		);
	}
}
#[rstest]
#[case("fixture-model")]
#[case("private.compaction")]
#[case("memory")]
#[case("tool")]
#[case("aidash.workspace_read")]
#[case("aidash.human_request")]
#[tokio::test]
async fn revocation_of_any_admitted_dependency_prevents_accepting_output(
	mut scope: Scope,
	run: RunMetadata,
	#[case] dependency: &str,
) {
	let path = if dependency == "private.compaction" {
		"metadata"
	} else {
		"catalog"
	};
	scope.deny = Some(format!("{path}:{dependency}:registry.read"));
	assert!(matches!(
		authorize(&mut scope, &run, true).await,
		Err(Error::Forbidden)
	));
}
#[rstest]
#[case("producer")]
#[case("fixture-model")]
#[case("private.compaction")]
#[case("memory")]
#[case("tool")]
#[tokio::test]
async fn a_changed_immutable_definition_cannot_replace_the_admitted_one(
	mut scope: Scope,
	run: RunMetadata,
	#[case] dependency: &str,
) {
	scope.changed = Some(dependency.into());
	assert!(matches!(
		authorize(&mut scope, &run, true).await,
		Err(Error::Conflict(_))
	));
}
#[rstest]
#[case("visible")]
#[case("clusters")]
#[case("task_read")]
#[case("task.execute")]
#[case("live")]
#[case("pinned")]
#[case("snapshot")]
#[case("context")]
#[tokio::test]
async fn adapter_fault_stops_at_the_failing_boundary(
	mut scope: Scope,
	run: RunMetadata,
	#[case] boundary: &str,
) {
	if boundary == "context" {
		let mut root = scope
			.snapshot
			.definitions
			.iter()
			.find(|d| d.identity == scope.snapshot.agent)
			.unwrap()
			.definition
			.clone();
		root.binding_normalization = None;
		root.config["remove_default"]
			.as_array_mut()
			.unwrap()
			.retain(|name| name != "file_read");
		let extras = scope
			.snapshot
			.definitions
			.iter()
			.filter(|d| ["memory", "tool", "private.compaction"].contains(&d.identity.id.as_str()))
			.map(|d| d.definition.clone())
			.collect();
		scope.snapshot = crate::test_support::resolve("aidash://local", &root, false, extras);
	}
	scope.fail = Some(boundary.into());
	assert!(matches!(
		authorize(&mut scope, &run, true).await,
		Err(Error::Port(_))
	));
	assert_eq!(scope.calls.last().unwrap(), boundary);
}
#[rstest]
#[tokio::test]
async fn hidden_run_stops_before_any_registry_disclosure(mut scope: Scope, run: RunMetadata) {
	scope.visible = false;
	assert!(matches!(
		authorize(&mut scope, &run, true).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, ["visible"]);
}
#[rstest]
#[tokio::test]
async fn conversation_cluster_remains_required_independently_of_agent_config(
	mut scope: Scope,
	run: RunMetadata,
) {
	scope.deny = Some("catalog:conversation-cluster:cluster.execute".into());
	assert!(matches!(
		authorize(&mut scope, &run, true).await,
		Err(Error::Forbidden)
	));
	assert!(!scope.calls.contains(&"task_read".into()));
}
#[rstest]
#[tokio::test]
async fn malformed_conversation_target_and_missing_agent_subject_are_rejected(
	mut scope: Scope,
	run: RunMetadata,
) {
	scope.clusters = vec!["malformed".into()];
	assert!(matches!(
		authorize(&mut scope, &run, true).await,
		Err(Error::Forbidden)
	));
	scope.clusters.clear();
	scope.bundle.subjects.clear();
	assert!(matches!(
		authorize(&mut scope, &run, true).await,
		Err(Error::Forbidden)
	));
	assert!(!scope.calls.contains(&"snapshot".into()));
}

#[rstest]
#[tokio::test]
async fn terminal_delivery_does_not_reopen_the_failed_inference_context(
	mut scope: Scope,
	run: RunMetadata,
) {
	scope.fail = Some("snapshot".into());
	authorize(&mut scope, &run, false).await.unwrap();
	assert!(!scope.calls.contains(&"snapshot".into()));
	assert!(!scope.calls.contains(&"context".into()));
	assert_eq!(scope.calls.last().unwrap(), "pinned");
	// The same broken context still prevents execution; delivery is not a
	// compatibility or reconstruction path for a new model/tool turn.
	assert!(matches!(
		authorize(&mut scope, &run, true).await,
		Err(Error::Port(_))
	));
}

#[rstest]
#[case("task.execute")]
#[case("catalog:producer:agent.execute")]
#[case("pinned")]
#[tokio::test]
async fn terminal_delivery_retains_current_task_agent_and_installation_authority(
	mut scope: Scope,
	run: RunMetadata,
	#[case] boundary: &str,
) {
	scope.deny = Some(boundary.into());
	assert!(matches!(
		authorize(&mut scope, &run, false).await,
		Err(Error::Forbidden)
	));
	assert!(!scope.calls.contains(&"snapshot".into()));
}

#[rstest]
#[case("foreign-child")]
#[case("producer")]
#[tokio::test]
async fn foreign_closures_do_not_require_local_namesakes_or_match_their_digests(
	mut scope: Scope,
	run: RunMetadata,
	#[case] child_id: &str,
) {
	use aidash_domain::registry::{
		bindings::{ForeignAgentSnapshot, ResolvedDefinition},
		rules::digest,
	};
	let mut child = crate::test_support::agent(child_id);
	child.config["instructions"] = json!("Instructions owned by the peer.");
	let foreign = ForeignAgentSnapshot::from_snapshot(crate::test_support::resolve(
		"aidash://peer",
		&child,
		true,
		vec![],
	))
	.unwrap();
	let saved = scope
		.snapshot
		.definitions
		.iter_mut()
		.find(|saved| {
			saved.identity.registry_deployment == "aidash://local" && saved.identity.id == "tool"
		})
		.unwrap();
	saved.definition.config = json!({
		"registry_deployment":"aidash://local","provider":"integration.agent@1","operation":"invoke",
		"default_alias":"lookup","tier":"integration",
		"transport":{"transport":"agent","node_id":foreign.agent.registry_deployment,"agent":foreign.agent.local()}
	});
	*saved = ResolvedDefinition::new(saved.identity.clone(), saved.definition.clone()).unwrap();
	let binding = scope
		.snapshot
		.bindings
		.iter_mut()
		.find(|binding| binding.identity == saved.identity)
		.unwrap();
	binding.definition = saved.definition.clone();
	binding.digest = saved.digest.clone();
	let descriptor: aidash_domain::tool::providers::ToolDescriptor =
		serde_json::from_value(binding.definition.config.clone()).unwrap();
	binding.provider_contract_digest = Some(digest(
		&serde_json::to_value(
			descriptor
				.declared_contract(binding.identity.clone())
				.unwrap(),
		)
		.unwrap(),
	));
	binding.provider_implementation = Some("integration.agent@1:portable-test".into());
	scope
		.snapshot
		.definitions
		.extend(foreign.definitions.clone());
	scope.snapshot.foreign_agents.push(foreign);
	scope.snapshot.validate().unwrap();
	let local_definitions = scope
		.snapshot
		.definitions
		.iter()
		.filter(|saved| saved.identity.registry_deployment == "aidash://local")
		.count();
	assert!(authorize(&mut scope, &run, true).await.is_ok());
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|call| call.ends_with(":registry.read"))
			.count(),
		local_definitions
	);
	assert!(
		!scope
			.calls
			.contains(&"catalog:foreign-child:registry.read".into())
	);
}

#[rstest]
#[tokio::test]
async fn a_delegated_agents_model_is_checked_against_that_agent(
	mut scope: Scope,
	run: RunMetadata,
) {
	// producer → agent tool → delegate → delegated-model (ADR 0095).
	let node = "aidash://local";
	let model = |id: &str| {
		crate::test_support::entry(
			id,
			"model",
			json!({"provider":"openrouter","model_id":"fixture","endpoint":"https://fixture.invalid","context_window":200000,"max_output_tokens":1024,"modalities":["text"],"cost":{}}),
		)
	};
	let mut producer = crate::test_support::agent("producer");
	producer.config["remove_default"] = json!(aidash_domain::registry::bindings::DEFAULT_TOOLS);
	producer.config["bindings"] = json!([
		{"kind":"tool","target":{"registry_deployment":node,"id":"delegation","version":"1.0.0"}}
	]);
	let mut delegate = crate::test_support::agent("delegate");
	delegate.config["model"] = json!({"id":"delegated-model","version":"1.0.0"});
	delegate.config["remove_default"] = json!(aidash_domain::registry::bindings::DEFAULT_TOOLS);
	let tool = crate::test_support::entry(
		"delegation",
		"tool",
		json!({"registry_deployment":node,"provider":"integration.agent@1","operation":"invoke","default_alias":"delegate","tier":"integration","transport":{"transport":"agent","node_id":node,"agent":{"id":"delegate","version":"1.0.0"}}}),
	);
	scope.snapshot = crate::test_support::resolve(
		node,
		&producer,
		false,
		vec![
			tool,
			delegate,
			model("fixture-model"),
			model("delegated-model"),
		],
	);
	authorize(&mut scope, &run, true).await.unwrap();
	assert!(
		scope
			.calls
			.contains(&"closure:delegated-model:registry.read:delegate".into()),
		"{:?}",
		scope.calls
	);
	for used in ["fixture-model", "delegation", "delegate"] {
		assert!(
			scope
				.calls
				.contains(&format!("catalog:{used}:registry.read")),
			"{used}: {:?}",
			scope.calls
		);
	}
}
