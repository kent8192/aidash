use super::*;
use crate::ports::catalog::CatalogMutation;
use aidash_domain::{
	identity::{
		catalog::{CatalogRestriction, RestrictionChange},
		tenancy::{MEMBERS, initial_policy},
	},
	policy::{PolicyBundle, SubjectKind},
};
use async_trait::async_trait;
use rstest::rstest;
use serde_json::{Value, json};

const NODE: &str = "aidash://node-a/brave-otter-k3m9q2xa";

struct Fixture {
	definitions: BTreeMap<(String, String), Entry>,
	approved: BTreeSet<(String, String)>,
	actions: BTreeSet<&'static str>,
	policy: PolicyBundle,
	writes: Vec<String>,
	saved: usize,
	node: aidash_domain::nodes::NodeAddress,
	stored_restriction: Option<CatalogRestriction>,
}

fn entry(value: Value) -> Entry {
	serde_json::from_value(value).unwrap()
}
fn agent(model: &str, tools: &[&str]) -> Entry {
	let bindings: Vec<_> = tools
		.iter()
		.map(
			|tool| json!({"kind":"tool","target":{"registry_deployment":"aidash://node-a","id":tool,"version":"1.0.0"}}),
		)
		.collect();
	entry(
		json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},
		"description":{"en":"Fixture"},
		"config":{"schema_version":1,"model":{"id":model,"version":"1.0.0"},"instructions":"Help","bindings":bindings,"remove_default":["compaction"]}}),
	)
}
/// An Agent that keeps the Node's default compaction Binding.
fn agent_with_default_compaction() -> Entry {
	let mut agent = agent("model", &[]);
	agent.config["remove_default"] = json!([]);
	agent
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
		restriction: None,
	}
}
fn subject() -> String {
	aidash_domain::qualified_agent(&node(), "agent", "1.0.0")
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
			node: node(),
			stored_restriction: None,
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
		node: &aidash_domain::nodes::NodeName,
		reference: &EntityRef,
		expected_revision: i64,
		enabled: bool,
		restriction: RestrictionChange,
	) -> Result<Option<Binding>> {
		self.writes
			.push(format!("approve:{}:{enabled}", reference.id));
		if let RestrictionChange::Replace(restriction) = restriction {
			self.stored_restriction = restriction;
		}
		Ok(Some(Binding {
			tenant: tenant.into(),
			node: node.clone(),
			entry_id: reference.id.clone(),
			entry_version: reference.version.clone(),
			enabled,
			revision: expected_revision + 1,
			restriction: self.stored_restriction.clone(),
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
	fn node(&self) -> &aidash_domain::nodes::NodeAddress {
		&self.node
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

fn node() -> aidash_domain::nodes::NodeAddress {
	NODE.parse().unwrap()
}

fn restricted(subjects: &[&str], groups: &[&str]) -> Option<CatalogRestriction> {
	Some(CatalogRestriction {
		subjects: subjects.iter().map(|value| (*value).to_owned()).collect(),
		groups: groups.iter().map(|value| (*value).to_owned()).collect(),
	})
}

#[rstest]
#[tokio::test]
async fn an_approval_records_a_restriction_of_existing_subjects_and_assignable_groups() {
	// Arrange
	let mut fixture = Fixture::new(vec![model()]);
	let mut request = approval("model", true);
	request.restriction = restricted(&["owner"], &["members"]);
	// Act
	let bindings = approve(&mut fixture, &[request]).await.unwrap();
	// Assert
	assert_eq!(
		bindings[0].restriction,
		restricted(&["owner"], &["members"])
	);
}

#[rstest]
#[tokio::test]
async fn a_restriction_naming_an_unknown_subject_is_refused_before_any_write() {
	// Arrange
	let mut fixture = Fixture::new(vec![model()]);
	let mut request = approval("model", true);
	request.restriction = restricted(&["nobody"], &[]);
	// Act
	let result = approve(&mut fixture, &[request]).await;
	// Assert
	assert!(
		matches!(&result, Err(Error::Invalid(message)) if message == "CATALOG_RESTRICTION_UNKNOWN_SUBJECT: nobody"),
		"{result:?}"
	);
	assert!(fixture.writes.is_empty());
}

#[rstest]
#[tokio::test]
async fn a_restriction_naming_a_group_that_is_not_assignable_is_refused() {
	// Arrange
	let mut fixture = Fixture::new(vec![model()]);
	let mut request = approval("model", true);
	request.restriction = restricted(&[], &["not-a-group"]);
	// Act
	let result = approve(&mut fixture, &[request]).await;
	// Assert
	assert!(
		matches!(&result, Err(Error::Invalid(message)) if message == "CATALOG_RESTRICTION_NOT_ASSIGNABLE: not-a-group"),
		"{result:?}"
	);
	assert!(fixture.writes.is_empty());
}

#[rstest]
#[tokio::test]
async fn an_omitted_restriction_keeps_the_stored_one_and_an_empty_one_clears_it() {
	// Arrange: an approval already restricted to owner.
	let mut fixture = Fixture::new(vec![model()]);
	fixture.stored_restriction = restricted(&["owner"], &[]);
	// Act: omit the restriction, then explicitly clear it.
	let kept = approve(&mut fixture, &[approval("model", true)])
		.await
		.unwrap();
	let mut cleared = approval("model", true);
	cleared.restriction = restricted(&[], &[]);
	let cleared = approve(&mut fixture, &[cleared]).await.unwrap();
	// Assert
	assert_eq!(kept[0].restriction, restricted(&["owner"], &[]));
	assert_eq!(cleared[0].restriction, None);
}

#[rstest]
#[tokio::test]
async fn a_restriction_may_name_the_agent_subject_this_request_creates() {
	// Arrange: the model dependency is approved in the same request.
	let mut fixture = Fixture::new(vec![agent("model", &[]), model()]);
	let mut dependency = approval("model", true);
	dependency.restriction = restricted(&[&subject()], &[]);
	// Act
	let bindings = approve(&mut fixture, &[approval("agent", true), dependency])
		.await
		.unwrap();
	// Assert
	assert_eq!(bindings[1].restriction, restricted(&[&subject()], &[]));
}

#[rstest]
#[tokio::test]
async fn the_default_compaction_decider_is_a_dependency_of_the_agent() {
	// Arrange
	let mut fixture = Fixture::new(vec![agent_with_default_compaction(), model()]);
	fixture.approved.insert(("model".into(), "1.0.0".into()));
	// Act
	let dependencies = dependencies(
		&agent_with_default_compaction(),
		fixture.node().deployment.as_str(),
	)
	.unwrap();
	let result = approve(&mut fixture, &[approval("agent", true)]).await;
	// Assert
	assert!(dependencies.contains(&reference("aidash.compaction-decider")));
	assert!(
		matches!(&result, Err(Error::Invalid(message)) if message == "CATALOG_DEPENDENCY_UNAPPROVED: aidash.compaction-decider@1.0.0"),
		"{result:?}"
	);
	assert!(fixture.writes.is_empty());
}

#[rstest]
#[tokio::test]
async fn an_agent_is_approved_once_its_compaction_decider_is_approved() {
	// Arrange
	let decider = crate::registry::system::compaction_decider().unwrap();
	let mut fixture = Fixture::new(vec![agent_with_default_compaction(), model(), decider]);
	fixture.approved.insert(("model".into(), "1.0.0".into()));
	// Act
	let together = approve(
		&mut fixture,
		&[
			approval("agent", true),
			approval("aidash.compaction-decider", true),
		],
	)
	.await;
	// Assert
	assert_eq!(together.unwrap().len(), 2);
	// Arrange
	let decider = crate::registry::system::compaction_decider().unwrap();
	let mut fixture = Fixture::new(vec![agent_with_default_compaction(), model(), decider]);
	fixture.approved.insert(("model".into(), "1.0.0".into()));
	fixture
		.approved
		.insert(("aidash.compaction-decider".into(), "1.0.0".into()));
	// Act
	let already = approve(&mut fixture, &[approval("agent", true)]).await;
	// Assert
	assert_eq!(already.unwrap().len(), 1);
}

#[rstest]
#[tokio::test]
async fn an_agent_without_the_default_has_no_compaction_dependency() {
	// Arrange
	let entry = agent("model", &[]);
	// Act
	let dependencies = dependencies(&entry, "aidash://node-a").unwrap();
	// Assert
	assert!(!dependencies.contains(&reference("aidash.compaction-decider")));
}
