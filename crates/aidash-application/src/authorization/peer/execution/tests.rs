use super::*;
use aidash_domain::{
	generation::remote::Ancestor,
	identity::execution::ExecutionPrincipal,
	policy::{PolicyBundle, Resource},
};
use async_trait::async_trait;
use rstest::rstest;
use serde_json::{Value, json};
use uuid::Uuid;

struct Scope {
	bundle: PolicyBundle,
	subjects: Vec<String>,
	context: Value,
	calls: Vec<(String, Vec<String>)>,
	active: bool,
	model_kind: String,
	denied_action: Option<String>,
}
impl Scope {
	fn new() -> Self {
		let executor = qualified_agent("aidash://receiver", "agent", "1.0.0");
		Self {
			bundle: serde_json::from_value(
				json!({"tenant":"local","subjects":{executor:{"kind":"agent","delegated_by":null}}}),
			)
			.unwrap(),
			subjects: vec!["mapped".into()],
			context: json!({}),
			calls: vec![],
			active: true,
			model_kind: "model".into(),
			denied_action: None,
		}
	}
	fn call(&mut self, name: &str) {
		self.calls.push((name.into(), self.subjects.clone()));
	}
}
#[async_trait]
impl PeerInspectionScope for Scope {
	fn node_id(&self) -> &str {
		"aidash://receiver"
	}
	fn identity(&self) -> ExecutionPrincipal {
		ExecutionPrincipal {
			tenant: "local".into(),
			subject: "mapped".into(),
			credential_id: Uuid::from_u128(1),
		}
	}
	fn bundle(&self) -> &PolicyBundle {
		&self.bundle
	}
	fn subjects(&self) -> &[String] {
		&self.subjects
	}
	fn push_subject(&mut self, subject: String) {
		self.subjects.push(subject);
	}
	fn context(&mut self) -> &mut Value {
		&mut self.context
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		Resource {
			tenant: "local".into(),
			kind: kind.into(),
			id: id.into(),
			attributes,
		}
	}
	async fn require(&mut self, _: &Resource, action: &str) -> Result<()> {
		self.call(action);
		Ok(())
	}
	async fn generation(&mut self, _: &str, _: &InspectInput) -> Result<Option<Value>> {
		self.call("generation");
		Ok(None)
	}
	async fn entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		self.call(action);
		if self.denied_action.as_deref() == Some(action) {
			return Err(Error::Forbidden);
		}
		let snapshot = crate::test_support::snapshot("aidash://receiver", "agent");
		let mut entry = snapshot
			.definitions
			.iter()
			.find(|d| d.identity.local() == *reference)
			.expect("admitted fixture definition")
			.definition
			.clone();
		if entry.kind == "model" {
			entry.kind = self.model_kind.clone();
		}
		Ok(entry)
	}
	async fn bindings(
		&mut self,
		_: &Entry,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
		Ok(crate::test_support::snapshot("aidash://receiver", "agent"))
	}
	fn entry_resource(&self, entry: &Entry) -> Resource {
		self.resource("registry", &entry.id, json!({}))
	}
	async fn active_installation(&mut self, _: &Entry) -> Result<bool> {
		self.call("active");
		Ok(self.active)
	}
	async fn pinned_installation(&mut self, _: &Entry) -> Result<()> {
		self.call("pinned");
		Ok(())
	}
	async fn lineage(&mut self) -> Result<Vec<Ancestor>> {
		self.call("lineage");
		Ok(vec![])
	}
}
fn input() -> InspectInput {
	serde_json::from_value(json!({"tenant":"source","subject":"human","agent":{"id":"agent","version":"1.0.0"},"requirements":{}})).unwrap()
}

#[rstest]
#[tokio::test]
async fn preflight_checks_receiver_and_delegated_executor_before_pinning_definitions() {
	let mut scope = Scope::new();
	let inspection = inspect(&mut scope, "aidash://home", &input())
		.await
		.unwrap();
	let executor = qualified_agent("aidash://receiver", "agent", "1.0.0");
	assert_eq!(
		scope.calls[0],
		("federation.execute".into(), vec!["mapped".into()])
	);
	assert_eq!(
		scope.calls[2],
		(
			"federation.execute".into(),
			vec!["mapped".into(), executor.clone()]
		)
	);
	assert_eq!(
		inspection.definitions.len(),
		inspection.binding_snapshot.definitions.len()
	);
	assert!(inspection.definitions.iter().any(|d| d.kind == "agent"));
	assert!(inspection.definitions.iter().any(|d| d.kind == "model"));
	assert!(inspection.definitions.iter().any(|d| d.kind == "tool"));
	assert_eq!(
		inspection.authority_digest,
		digest(
			&json!({"source_node":"aidash://home","source_tenant":"source","source_subject":"human","tenant":"local","credential_id":Uuid::from_u128(1),"subjects":["mapped",executor]})
		)
	);
}

#[rstest]
#[tokio::test]
async fn missing_executor_authority_stops_before_generation_or_catalog_access() {
	let mut scope = Scope::new();
	scope.bundle.subjects.clear();
	assert!(matches!(
		inspect(&mut scope, "aidash://home", &input()).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls.len(), 1);
	assert_eq!(scope.calls[0].0, "federation.execute");
}

#[rstest]
#[tokio::test]
async fn inactive_installation_stops_before_model_or_pinned_dependency_use() {
	let mut scope = Scope::new();
	scope.active = false;
	assert!(matches!(
		inspect(&mut scope, "aidash://home", &input()).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls.last().unwrap().0, "active");
	assert!(
		!scope
			.calls
			.iter()
			.any(|(name, _)| name == "pinned" || name == "model.infer")
	);
}

#[rstest]
#[tokio::test]
async fn wrong_dependency_kind_cannot_become_an_admitted_definition() {
	let mut scope = Scope::new();
	scope.model_kind = "tool".into();
	let result = inspect(&mut scope, "aidash://home", &input()).await;
	assert!(
		matches!(result, Err(Error::Invalid(ref message)) if message=="executor dependency has the wrong kind")
	);
	assert!(!scope.calls.iter().any(|(name, _)| name == "lineage"));
}

#[rstest]
#[tokio::test]
async fn denied_bound_tool_invocation_prevents_remote_preflight() {
	let mut scope = Scope::new();
	scope.denied_action = Some("tool.invoke".into());
	assert!(matches!(
		inspect(&mut scope, "aidash://home", &input()).await,
		Err(Error::Forbidden)
	));
	let (action, subjects) = scope.calls.last().unwrap();
	assert_eq!(action, "tool.invoke");
	assert_eq!(
		subjects,
		&[
			"mapped".to_owned(),
			qualified_agent("aidash://receiver", "agent", "1.0.0")
		]
	);
	assert!(!scope.calls.iter().any(|(name, _)| name == "lineage"));
}
