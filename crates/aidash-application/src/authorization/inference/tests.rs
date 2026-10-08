use super::*;
use aidash_domain::{RunControl, RunPhase, policy::Resource};
use async_trait::async_trait;
use chrono::Utc;
use rstest::{fixture, rstest};
use serde_json::json;
use uuid::Uuid;
fn id(value: u128) -> Uuid {
	Uuid::from_u128(value)
}
fn reference(key: &str) -> EntityRef {
	EntityRef {
		id: key.into(),
		version: "1".into(),
	}
}
#[fixture]
fn agent() -> AgentConfig {
	let mut config: AgentConfig =
		serde_json::from_value(crate::test_support::agent("fixture").config).unwrap();
	config.model = reference("model");
	config.core_capabilities = Default::default();
	config.core_capabilities.files = true;
	config.skills = vec![reference("first"), reference("second")];
	config.conversation_memory = true;
	config.memory = Some(reference("memory"));
	config.allow_cross_conversation_memory = Some(true);
	config
}

#[fixture]
fn run() -> RunMetadata {
	RunMetadata {
		id: id(1),
		task_id: id(2),
		workspace_id: id(3),
		home_node: "aidash://home".into(),
		agent_id: "producer".into(),
		agent_version: "2".into(),
		phase: RunPhase::Ready,
		control: RunControl::Active,
		step: 3,
		revision: 7,
		observed_input_seq: 9,
		ledger_worker_ready: true,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: Utc::now(),
	}
}
struct Scope {
	remote: bool,
	node: Option<String>,
	calls: Vec<String>,
	fail: Option<&'static str>,
	deny: Option<&'static str>,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		remote: false,
		node: Some("aidash://current".into()),
		calls: vec![],
		fail: None,
		deny: None,
	}
}
impl Scope {
	fn record(&mut self, name: &str) -> Result<()> {
		self.calls.push(name.into());
		if self.deny == Some(name) {
			return Err(Error::Forbidden);
		}
		if self.fail == Some(name) {
			return Err(Error::Port(Box::new(std::io::Error::other(format!(
				"{name} fault"
			)))));
		}
		Ok(())
	}
}
#[async_trait]
impl InferenceScope for Scope {
	fn remote(&self) -> bool {
		self.remote
	}
	fn node_id(&self) -> Option<&str> {
		self.node.as_deref()
	}
	async fn context_authority(&mut self, run: &RunMetadata) -> Result<()> {
		assert_eq!(run.id, id(1));
		self.record("context")
	}
	async fn require_live(
		&mut self,
		node: &str,
		run: &RunMetadata,
		agent: &EntityRef,
	) -> Result<()> {
		assert_eq!(node, "aidash://current");
		assert_eq!(run.task_id, id(2));
		assert_eq!(
			agent,
			&EntityRef {
				id: "producer".into(),
				version: "2".into()
			}
		);
		self.record("live")
	}
	async fn catalog(&mut self, reference: &EntityRef, action: &str) -> Result<()> {
		self.record(&format!("{}:{action}", reference.id))
	}
	async fn memory_resource(&mut self, run: &RunMetadata) -> Result<Resource> {
		assert_eq!(run.workspace_id, id(3));
		self.record("memory")?;
		Ok(Resource {
			tenant: "tenant".into(),
			kind: "memory".into(),
			id: "saved memory scope".into(),
			attributes: json!({"current":true}),
		})
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		assert_eq!(resource.id, "saved memory scope");
		assert_eq!(resource.attributes, json!({"current":true}));
		self.record(action)
	}
}
fn expected() -> Vec<&'static str> {
	vec![
		"context",
		"live",
		"model:model.infer",
		"first:skill.use",
		"second:skill.use",
		"memory",
		"memory.read",
	]
}
#[rstest]
#[tokio::test]
async fn current_local_inference_checks_core_lineage_model_skills_and_saved_memory(
	mut scope: Scope,
	run: RunMetadata,
	agent: AgentConfig,
) {
	authorize(&mut scope, &run, &agent).await.unwrap();
	assert_eq!(scope.calls, expected());
}
#[rstest]
#[case::unspecified(None)]
#[case::allowed(Some(true))]
#[case::denied(Some(false))]
#[tokio::test]
async fn memory_reads_require_explicit_opt_in(
	mut scope: Scope,
	run: RunMetadata,
	mut agent: AgentConfig,
	#[case] setting: Option<bool>,
) {
	agent.conversation_memory = setting == Some(true);
	authorize(&mut scope, &run, &agent).await.unwrap();
	assert_eq!(
		scope.calls,
		if setting != Some(true) {
			expected()[..5].to_vec()
		} else {
			expected()
		}
	);
}
#[rstest]
#[tokio::test]
async fn disabled_core_capabilities_skip_only_context_authority(
	mut scope: Scope,
	run: RunMetadata,
	mut agent: AgentConfig,
) {
	agent.core_capabilities.files = false;

	authorize(&mut scope, &run, &agent).await.unwrap();
	assert_eq!(scope.calls, expected()[1..]);
}
#[rstest]
#[case::core_enabled(true)]
#[case::core_disabled(false)]
#[tokio::test]
async fn missing_current_node_denies_local_inference_before_lineage_or_provider_access(
	mut scope: Scope,
	run: RunMetadata,
	mut agent: AgentConfig,
	#[case] core: bool,
) {
	scope.node = None;
	agent.core_capabilities.files = core;
	agent.conversation_memory = false;
	assert!(matches!(
		authorize(&mut scope, &run, &agent).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, if core { vec!["context"] } else { vec![] });
}
#[rstest]
#[case::memory_enabled(Some(true))]
#[case::memory_disabled(Some(false))]
#[case::memory_default(None)]
#[tokio::test]
async fn remote_inference_checks_model_and_skills_without_local_context_lineage_or_memory(
	mut scope: Scope,
	run: RunMetadata,
	mut agent: AgentConfig,
	#[case] memory: Option<bool>,
) {
	scope.remote = true;
	scope.node = None;
	agent.conversation_memory = memory != Some(false);
	authorize(&mut scope, &run, &agent).await.unwrap();
	assert_eq!(scope.calls, expected()[2..5]);
}
#[rstest]
#[tokio::test]
async fn repeated_skill_dependencies_keep_their_order_and_every_current_decision(
	mut scope: Scope,
	run: RunMetadata,
	mut agent: AgentConfig,
) {
	agent.skills = vec![reference("second"), reference("first"), reference("second")];
	authorize(&mut scope, &run, &agent).await.unwrap();
	assert_eq!(
		scope.calls,
		vec![
			"context",
			"live",
			"model:model.infer",
			"second:skill.use",
			"first:skill.use",
			"second:skill.use",
			"memory",
			"memory.read"
		]
	);
}
#[rstest]
#[case::model("model:model.infer")]
#[case::skill("first:skill.use")]
#[case::memory("memory.read")]
#[tokio::test]
async fn revocation_at_any_current_resource_prevents_inference(
	mut scope: Scope,
	run: RunMetadata,
	agent: AgentConfig,
	#[case] action: &'static str,
) {
	scope.deny = Some(action);
	assert!(matches!(
		authorize(&mut scope, &run, &agent).await,
		Err(Error::Forbidden)
	));
	let expected = expected();
	let last = expected.iter().position(|name| *name == action).unwrap();
	assert_eq!(scope.calls, expected[..=last]);
}
#[rstest]
#[case::context("context")]
#[case::lineage("live")]
#[case::model("model:model.infer")]
#[case::first_skill("first:skill.use")]
#[case::second_skill("second:skill.use")]
#[case::memory_resource("memory")]
#[case::memory_read("memory.read")]
#[tokio::test]
async fn adapter_faults_preserve_the_boundary_without_reading_later_resources(
	mut scope: Scope,
	run: RunMetadata,
	agent: AgentConfig,
	#[case] boundary: &'static str,
) {
	scope.fail = Some(boundary);
	let Error::Port(error) = authorize(&mut scope, &run, &agent).await.err().unwrap() else {
		panic!("expected inference authority fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		format!("{boundary} fault")
	);
	let expected = expected();
	let last = expected.iter().position(|name| *name == boundary).unwrap();
	assert_eq!(scope.calls, expected[..=last]);
}
