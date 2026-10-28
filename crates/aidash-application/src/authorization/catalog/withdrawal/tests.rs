use super::*;
use aidash_domain::{nodes::NodeAddress, registry::Entry};
use async_trait::async_trait;
use rstest::rstest;
use serde_json::{Value, json};

const NODE: &str = "aidash://node-a/brave-otter-k3m9q2xa";

struct Fixture {
	definitions: Vec<Entry>,
	approved_agents: Vec<Entry>,
	may_approve: bool,
	runs: u64,
	run_queries: Vec<Vec<EntityRef>>,
	node: NodeAddress,
}

fn entry(value: Value) -> Entry {
	serde_json::from_value(value).unwrap()
}
fn agent(id: &str, remove_default: &[&str]) -> Entry {
	entry(
		json!({"id":id,"version":"1.0.0","kind":"agent","name":{"en":id},
		"description":{"en":"Fixture"},
		"config":{"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Help","bindings":[],"remove_default":remove_default}}),
	)
}
fn reference(id: &str) -> EntityRef {
	EntityRef {
		id: id.into(),
		version: "1.0.0".into(),
	}
}
fn decider() -> EntityRef {
	reference("aidash.compaction-decider")
}

impl Fixture {
	fn new(approved_agents: Vec<Entry>) -> Self {
		let mut definitions = approved_agents.clone();
		definitions.push(crate::registry::system::compaction_decider().unwrap());
		Self {
			definitions,
			approved_agents,
			may_approve: true,
			runs: 3,
			run_queries: vec![],
			node: NODE.parse().unwrap(),
		}
	}
}

#[async_trait]
impl WithdrawalImpactScope for Fixture {
	fn node(&self) -> &NodeAddress {
		&self.node
	}
	async fn definition(&mut self, reference: &EntityRef) -> Result<Option<Entry>> {
		Ok(self
			.definitions
			.iter()
			.find(|entry| entry.id == reference.id && entry.version == reference.version)
			.cloned())
	}
	async fn can_approve(&mut self, _: &Entry) -> Result<bool> {
		Ok(self.may_approve)
	}
	async fn approved_agents(&mut self) -> Result<Vec<Entry>> {
		Ok(self.approved_agents.clone())
	}
	async fn non_terminal_runs(&mut self, agents: &[EntityRef]) -> Result<u64> {
		self.run_queries.push(agents.to_vec());
		Ok(self.runs)
	}
}

#[rstest]
#[tokio::test]
async fn withdrawing_the_decider_lists_agents_that_keep_the_default_binding() {
	// Arrange
	let mut fixture = Fixture::new(vec![
		agent("b", &[]),
		agent("a", &[]),
		agent("none", &["compaction"]),
	]);
	// Act
	let impact = impact(&mut fixture, &decider()).await.unwrap();
	// Assert
	assert_eq!(impact.agents, [reference("a"), reference("b")]);
	assert_eq!(impact.running_runs, 3);
	assert_eq!(fixture.run_queries, [[reference("a"), reference("b")]]);
}

#[rstest]
#[tokio::test]
async fn an_unreferenced_version_has_no_impact_and_reads_no_runs() {
	// Arrange
	let mut fixture = Fixture::new(vec![agent("none", &["compaction"])]);
	// Act
	let impact = impact(&mut fixture, &decider()).await.unwrap();
	// Assert
	assert_eq!(impact, WithdrawalImpact::default());
	assert!(fixture.run_queries.is_empty());
}

#[rstest]
#[tokio::test]
async fn reading_the_impact_needs_the_right_to_withdraw() {
	// Arrange
	let mut fixture = Fixture::new(vec![agent("a", &[])]);
	fixture.may_approve = false;
	// Act
	let result = impact(&mut fixture, &decider()).await;
	// Assert
	assert!(matches!(result, Err(Error::Forbidden)), "{result:?}");
	assert!(fixture.run_queries.is_empty());
}

#[rstest]
#[tokio::test]
async fn an_unknown_version_is_not_found() {
	// Arrange
	let mut fixture = Fixture::new(vec![]);
	// Act
	let result = impact(&mut fixture, &reference("missing")).await;
	// Assert
	assert!(matches!(result, Err(Error::NotFound(_))), "{result:?}");
}
