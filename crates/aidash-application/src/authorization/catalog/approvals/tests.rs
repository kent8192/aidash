use super::*;
use crate::ports::catalog::CatalogMutation;
use aidash_domain::{
	identity::tenancy::{MEMBERS, initial_policy},
	policy::{PolicyBundle, SubjectKind},
};
use async_trait::async_trait;
use rstest::rstest;
use serde_json::{Value, json};

const NODE: &str = "aidash://node-a";

struct Fixture {
	definitions: BTreeMap<(String, String), Entry>,
	approved: BTreeSet<(String, String)>,
	actions: BTreeSet<&'static str>,
	policy: PolicyBundle,
	writes: Vec<String>,
	saved: usize,
}

fn entry(value: Value) -> Entry {
	serde_json::from_value(value).unwrap()
}
fn agent(model: &str, tools: &[&str]) -> Entry {
	let bindings: Vec<_> = tools
		.iter()
		.map(
			|tool| json!({"kind":"tool","target":{"registry_deployment":NODE,"id":tool,"version":"1.0.0"}}),
		)
		.collect();
	entry(
		json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},
		"description":{"en":"Fixture"},
		"config":{"schema_version":1,"model":{"id":model,"version":"1.0.0"},"instructions":"Help","bindings":bindings}}),
	)
}
fn model() -> Entry {
	entry(
		json!({"id":"model","version":"1.0.0","kind":"model","name":{"en":"Model"},
		"description":{"en":"Fixture"},"config":{}}),
	)
}
fn reference(id: &str) -> EntityRef {
	EntityRef {
		id: id.into(),
		version: "1.0.0".into(),
	}
}
fn approval(id: &str, enabled: bool) -> Approval {
	Approval {
		entry: reference(id),
		enabled,
		expected_revision: 0,
		groups: None,
	}
}
fn subject() -> String {
	aidash_domain::qualified_agent(NODE, "agent", "1.0.0")
}

impl Fixture {
	fn new(entries: Vec<Entry>) -> Self {
		Self {
			definitions: entries
				.into_iter()
				.map(|entry| ((entry.id.clone(), entry.version.clone()), entry))
				.collect(),
			approved: BTreeSet::new(),
			actions: [CATALOG_APPROVE].into(),
			policy: initial_policy("acme", true).unwrap(),
			writes: vec![],
			saved: 0,
		}
	}
}

#[async_trait]
impl CatalogMutation for Fixture {
	async fn registered(&mut self, reference: &EntityRef) -> Result<bool> {
		Ok(self
			.definitions
			.contains_key(&(reference.id.clone(), reference.version.clone())))
	}
	async fn compare_and_set(
		&mut self,
		tenant: &str,
		reference: &EntityRef,
		expected_revision: i64,
		enabled: bool,
	) -> Result<Option<Binding>> {
		self.writes
			.push(format!("approve:{}:{enabled}", reference.id));
		Ok(Some(Binding {
			tenant: tenant.into(),
			entry_id: reference.id.clone(),
			entry_version: reference.version.clone(),
			enabled,
			revision: expected_revision + 1,
		}))
	}
	async fn history(&mut self, binding: &Binding, actor: &str) -> Result<()> {
		self.writes
			.push(format!("history:{}:{actor}", binding.entry_id));
		Ok(())
	}
}

#[async_trait]
impl CatalogApprovalScope for Fixture {
	fn tenant(&self) -> &str {
		"acme"
	}
	fn node(&self) -> &str {
		NODE
	}
	fn actor(&self) -> &str {
		"owner"
	}
	async fn definition(&mut self, reference: &EntityRef) -> Result<Entry> {
		self.definitions
			.get(&(reference.id.clone(), reference.version.clone()))
			.cloned()
			.ok_or_else(|| Error::NotFound("definition".into()))
	}
	async fn approved(&mut self, reference: &EntityRef) -> Result<bool> {
		Ok(self
			.approved
			.contains(&(reference.id.clone(), reference.version.clone())))
	}
	async fn require(&mut self, _: &Entry, action: &str) -> Result<()> {
		self.actions
			.contains(action)
			.then_some(())
			.ok_or(Error::Forbidden)
	}
	async fn require_subject(&mut self, _: &str, action: &str, _: &BTreeSet<String>) -> Result<()> {
		self.actions
			.contains(action)
			.then_some(())
			.ok_or(Error::Forbidden)
	}
	fn policy(&mut self) -> &mut PolicyBundle {
		&mut self.policy
	}
	async fn save_policy(&mut self) -> Result<()> {
		self.saved += 1;
		Ok(())
	}
}

#[rstest]
#[tokio::test]
async fn an_approval_with_an_unapproved_dependency_is_refused_without_writes() {
	// Arrange
	let mut fixture = Fixture::new(vec![agent("model", &[]), model()]);
	// Act
	let result = approve(&mut fixture, &[approval("agent", true)]).await;
	// Assert
	assert!(
		matches!(&result, Err(Error::Invalid(message)) if message == "CATALOG_DEPENDENCY_UNAPPROVED: model@1.0.0"),
		"{result:?}"
	);
	assert!(fixture.writes.is_empty());
	assert!(!fixture.policy.subjects.contains_key(&subject()));
}

#[rstest]
#[tokio::test]
async fn one_request_approves_a_closure_and_makes_the_agent_a_member() {
	// Arrange
	let mut fixture = Fixture::new(vec![agent("model", &[]), model()]);
	// Act
	let bindings = approve(
		&mut fixture,
		&[approval("agent", true), approval("model", true)],
	)
	.await
	.unwrap();
	// Assert
	assert_eq!(bindings.len(), 2);
	assert_eq!(
		fixture.writes,
		[
			"approve:agent:true",
			"history:agent:owner",
			"approve:model:true",
			"history:model:owner"
		]
	);
	let created = &fixture.policy.subjects[&subject()];
	assert_eq!(created.kind, SubjectKind::Agent);
	assert_eq!(created.groups, [MEMBERS.to_owned()].into());
	assert_eq!(created.delegated_by, None);
	assert_eq!(fixture.saved, 1);
}

#[rstest]
#[tokio::test]
async fn built_ins_and_approved_versions_satisfy_dependencies() {
	// Arrange
	let mut fixture = Fixture::new(vec![agent("model", &["aidash.workspace_read"]), model()]);
	fixture.approved.insert(("model".into(), "1.0.0".into()));
	// Act
	let result = approve(&mut fixture, &[approval("agent", true)]).await;
	// Assert
	assert!(result.is_ok(), "{result:?}");
}

#[rstest]
#[tokio::test]
async fn approving_without_catalog_approve_is_refused() {
	// Arrange
	let mut fixture = Fixture::new(vec![model()]);
	fixture.actions.clear();
	// Act
	let result = approve(&mut fixture, &[approval("model", true)]).await;
	// Assert
	assert!(matches!(result, Err(Error::Forbidden)), "{result:?}");
	assert!(fixture.writes.is_empty());
}

#[rstest]
#[case::without_group_update(false, false)]
#[case::with_group_update(true, true)]
#[tokio::test]
async fn other_groups_need_subject_group_update(#[case] holds: bool, #[case] created: bool) {
	// Arrange
	let mut fixture = Fixture::new(vec![agent("model", &[]), model()]);
	fixture.approved.insert(("model".into(), "1.0.0".into()));
	if holds {
		fixture.actions.insert(rules::SUBJECT_GROUP_UPDATE);
	}
	let mut request = approval("agent", true);
	request.groups = Some(["administrators".to_owned()].into());
	// Act
	let result = approve(&mut fixture, &[request]).await;
	// Assert
	assert_eq!(result.is_ok(), created, "{result:?}");
	assert_eq!(fixture.policy.subjects.contains_key(&subject()), created);
}

#[rstest]
#[tokio::test]
async fn approving_again_never_changes_an_existing_agent_subject() {
	// Arrange
	let mut fixture = Fixture::new(vec![agent("model", &[]), model()]);
	fixture.approved.insert(("model".into(), "1.0.0".into()));
	approve(&mut fixture, &[approval("agent", true)])
		.await
		.unwrap();
	fixture.policy.subjects.get_mut(&subject()).unwrap().enabled = false;
	let mut named = approval("agent", true);
	named.groups = Some([MEMBERS.to_owned()].into());
	// Act
	let again = approve(&mut fixture, &[approval("agent", true)]).await;
	let renamed = approve(&mut fixture, &[named]).await;
	// Assert
	assert!(again.is_ok(), "{again:?}");
	assert!(!fixture.policy.subjects[&subject()].enabled);
	assert!(matches!(renamed, Err(Error::Conflict(_))), "{renamed:?}");
	assert_eq!(fixture.saved, 1);
}

#[rstest]
#[tokio::test]
async fn withdrawing_a_dependency_is_not_blocked() {
	// Arrange
	let mut fixture = Fixture::new(vec![agent("model", &[]), model()]);
	fixture.approved.insert(("model".into(), "1.0.0".into()));
	// Act
	let result = approve(&mut fixture, &[approval("model", false)]).await;
	// Assert
	assert!(result.is_ok(), "{result:?}");
	assert_eq!(fixture.writes[0], "approve:model:false");
}

#[rstest]
#[tokio::test]
async fn a_dependency_withdrawn_in_the_same_request_is_unapproved() {
	// Arrange
	let mut fixture = Fixture::new(vec![agent("model", &[]), model()]);
	fixture.approved.insert(("model".into(), "1.0.0".into()));
	// Act
	let result = approve(
		&mut fixture,
		&[approval("agent", true), approval("model", false)],
	)
	.await;
	// Assert
	assert!(
		matches!(&result, Err(Error::Invalid(message)) if message == "CATALOG_DEPENDENCY_UNAPPROVED: model@1.0.0"),
		"{result:?}"
	);
	assert!(fixture.writes.is_empty());
}

#[rstest]
#[tokio::test]
async fn an_agents_context_summarizer_is_a_dependency() {
	// Arrange
	let mut summarizing = agent("model", &[]);
	summarizing.config["context_policy"] = json!({"version":"context-recovery/1",
		"summary":{"model":{"id":"summarizer","version":"1.0.0"}}});
	let mut fixture = Fixture::new(vec![summarizing, model()]);
	fixture.approved.insert(("model".into(), "1.0.0".into()));
	// Act
	let result = approve(&mut fixture, &[approval("agent", true)]).await;
	// Assert
	assert!(
		matches!(&result, Err(Error::Invalid(message)) if message == "CATALOG_DEPENDENCY_UNAPPROVED: summarizer@1.0.0"),
		"{result:?}"
	);
	assert!(fixture.writes.is_empty());
}

#[rstest]
#[tokio::test]
async fn an_agent_whose_subject_has_another_kind_is_refused() {
	// Arrange
	let mut fixture = Fixture::new(vec![agent("model", &[]), model()]);
	fixture.approved.insert(("model".into(), "1.0.0".into()));
	fixture.policy.subjects.insert(
		subject(),
		serde_json::from_value(json!({"kind":"user"})).unwrap(),
	);
	// Act
	let result = approve(&mut fixture, &[approval("agent", true)]).await;
	// Assert
	assert!(
		matches!(&result, Err(Error::Conflict(message)) if message == "AGENT_SUBJECT_KIND"),
		"{result:?}"
	);
	assert_eq!(fixture.policy.subjects[&subject()].kind, SubjectKind::User);
	assert_eq!(fixture.saved, 0);
}
