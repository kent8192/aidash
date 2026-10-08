use super::*;
use aidash_domain::{
	federation::execution::Definition,
	policy::{PolicyBundle, Resource},
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use rstest::rstest;
use serde_json::{Value, json};
use uuid::Uuid;
struct Scope {
	subjects: Vec<String>,
	bundle: PolicyBundle,
	calls: Vec<(String, Resource, Value)>,
	context: Value,
	denied: Option<&'static str>,
	generation_checked: bool,
}
fn entry(kind: &str) -> Entry {
	serde_json::from_value(
		json!({"id":kind,"version":"1.0.0","kind":kind,"name":{},"description":{},"config":{}}),
	)
	.unwrap()
}
fn inspection(kind: &str) -> Inspection {
	let agent = entry("agent");
	let metadata = entry(kind);
	Inspection {
		binding_snapshot: crate::test_support::snapshot("aidash://receiver", "agent"),
		node_id: "aidash://receiver".into(),
		authority_digest: "sha256:pinned".into(),
		generation: None,
		lineage: vec![],
		agent,
		definitions: vec![Definition {
			entry: EntityRef {
				id: metadata.id.clone(),
				version: metadata.version.clone(),
			},
			kind: kind.into(),
			digest: "sha256:definition".into(),
			metadata,
		}],
		semantic_memory: 0,
		compactor: None,
	}
}
fn task() -> Task {
	serde_json::from_value(json!({"id":Uuid::from_u128(1),"workspace_id":Uuid::from_u128(2),"title":"title","description":"intent","status":"OPEN","requirements":{},"owner":null,"created_by":"requester","dependencies":[],"parent_id":null,"revision":1,"created_at":"2026-10-04T00:00:00Z"})).unwrap()
}
impl Scope {
	fn new() -> Self {
		let executor = qualified_agent("aidash://receiver", "agent", "1.0.0");
		Self {
			subjects: vec!["requester".into(), executor.clone()],
			bundle: serde_json::from_value(
				json!({"tenant":"tenant","subjects":{executor:{"kind":"agent","delegated_by":null}}}),
			)
			.unwrap(),
			calls: vec![],
			context: json!({}),
			denied: None,
			generation_checked: false,
		}
	}
}
#[async_trait]
impl SourceAuthorityScope for Scope {
	fn source_subjects(&self) -> &[String] {
		&self.subjects
	}
	fn source_bundle(&self) -> &PolicyBundle {
		&self.bundle
	}
	fn source_context(&mut self, attributes: Value) {
		self.context = attributes;
	}
	fn source_resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		Resource {
			tenant: "tenant".into(),
			kind: kind.into(),
			id: id.into(),
			attributes,
		}
	}
	async fn generation_home(&mut self, _: &Task, _: &str, _: Option<&Value>) -> Result<()> {
		self.generation_checked = true;
		Ok(())
	}
	async fn source_workspace(&mut self, id: Uuid) -> Result<Resource> {
		Ok(self.source_resource("workspace", &id.to_string(), json!({"workspace_id":id})))
	}
	async fn source_task_resource(&mut self, task: &Task) -> Result<Resource> {
		Ok(self.source_resource("task", &task.id.to_string(), self.context.clone()))
	}
	async fn source_require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.calls
			.push((action.into(), resource.clone(), self.context.clone()));
		if self.denied == Some(action) {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
}
#[rstest]
#[case("agent", "agent.execute")]
#[case("model", "model.infer")]
#[case("tool", "tool.invoke")]
#[case("skill", "skill.use")]
#[case("cluster", "cluster.execute")]
#[case("compactor", "compaction.invoke")]
#[tokio::test]
async fn exact_receiver_dependencies_require_read_and_execution_authority(
	#[case] kind: &str,
	#[case] action: &str,
) {
	let mut scope = Scope::new();
	authorize(&mut scope, &task(), "aidash://receiver", &inspection(kind))
		.await
		.unwrap();
	assert_eq!(
		scope
			.calls
			.iter()
			.map(|(action, _, _)| action.as_str())
			.collect::<Vec<_>>(),
		vec![
			"workspace.read",
			"task.read",
			"task.delegate",
			"task.execute",
			"federation.execute",
			"registry.read",
			action
		]
	);
	let (_, resource, context) = scope.calls.last().unwrap();
	assert_eq!(
		resource.id,
		format!("aidash://receiver/{kind}s/{kind}@1.0.0")
	);
	assert_eq!(
		resource.attributes["remote_node"],
		json!("aidash://receiver")
	);
	assert_eq!(resource.attributes["digest"], json!("sha256:definition"));
	assert_eq!(*context, json!({"workspace_id":task().workspace_id}));
	assert!(scope.generation_checked);
}
#[rstest]
#[case("workspace.read")]
#[case("task.read")]
#[case("task.delegate")]
#[case("task.execute")]
#[case("federation.execute")]
#[case("registry.read")]
#[case("model.infer")]
#[tokio::test]
async fn source_denial_stops_the_remaining_dependency_checks(#[case] denied: &'static str) {
	let mut scope = Scope::new();
	scope.denied = Some(denied);
	assert!(matches!(
		authorize(
			&mut scope,
			&task(),
			"aidash://receiver",
			&inspection("model")
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls.last().unwrap().0, denied);
}
#[rstest]
#[case("chain")]
#[case("missing")]
#[case("kind")]
#[tokio::test]
async fn executor_must_be_the_current_last_agent_subject(#[case] change: &str) {
	let mut scope = Scope::new();
	let executor = scope.subjects.last().unwrap().clone();
	match change {
		"chain" => scope.subjects.reverse(),
		"missing" => scope.bundle.subjects.clear(),
		"kind" => scope.bundle.subjects.get_mut(&executor).unwrap().kind = SubjectKind::User,
		_ => panic!("unknown authority"),
	}
	assert!(matches!(
		authorize(
			&mut scope,
			&task(),
			"aidash://receiver",
			&inspection("model")
		)
		.await,
		Err(Error::Forbidden)
	));
	assert!(scope.calls.is_empty());
	assert!(scope.generation_checked);
}
#[rstest]
#[tokio::test]
async fn unsupported_receiver_kind_cannot_gain_execution_authority() {
	let mut scope = Scope::new();
	assert!(matches!(
		authorize(
			&mut scope,
			&task(),
			"aidash://receiver",
			&inspection("future-provider")
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls.last().unwrap().0, "registry.read");
}

#[rstest]
#[case("memory")]
#[case("source")]
#[case("bundle")]
#[case("embedding")]
#[case("reranker")]
#[case("tokenizer")]
#[tokio::test]
async fn pinned_context_dependencies_preserve_source_registry_authority(
	#[case] kind: &str,
	#[values(false, true)] denied: bool,
) {
	let mut scope = Scope::new();
	if denied {
		scope.denied = Some("registry.read");
	}
	let result = authorize(&mut scope, &task(), "aidash://receiver", &inspection(kind)).await;
	if denied {
		assert!(matches!(result, Err(Error::Forbidden)));
	} else {
		result.unwrap();
	}
	assert_eq!(scope.calls.last().unwrap().0, "registry.read");
	assert_eq!(scope.calls.last().unwrap().1.kind, kind);
}

mod semantic;

mod reads;
