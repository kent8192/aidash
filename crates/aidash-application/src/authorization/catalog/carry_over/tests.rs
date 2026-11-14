use super::*;
use serde_json::json;

fn model(version: &str, provider: &str) -> Entry {
	serde_json::from_value(json!({"id":"model","version":version,"kind":"model","name":{},"description":{},"config":{"provider":provider,"model_id":"m","endpoint":"https://example.com/v1","context_window":100,"modalities":[],"cost":{}}})).unwrap()
}
fn node() -> NodeAddress {
	"aidash://local/brave-otter-k3m9q2xa".parse().unwrap()
}
fn source(version: &str, carry_over: bool) -> Source {
	Source {
		definition: model(version, "provider"),
		enabled: true,
		carry_over,
		scope: ApprovalScope::Node,
		approver: "alice".into(),
	}
}
#[derive(Default)]
struct Fixture {
	sources: Vec<Source>,
	writes: Vec<(String, String)>,
	scopes: Vec<ApprovalScope>,
	approved: bool,
	/// Nodes beyond the first; a source listed here is missing on that Node.
	other_nodes: Vec<(NodeAddress, Vec<Source>)>,
}
#[async_trait]
impl CarryScope for Fixture {
	async fn nodes(&mut self) -> Result<Vec<NodeAddress>> {
		let mut nodes = vec![node()];
		nodes.extend(self.other_nodes.iter().map(|(node, _)| node.clone()));
		Ok(nodes)
	}
	async fn sources(&mut self, at: &NodeAddress, _: &str) -> Result<Vec<Source>> {
		Ok(self
			.other_nodes
			.iter()
			.find(|(node, _)| node == at)
			.map_or_else(|| self.sources.clone(), |(_, sources)| sources.clone()))
	}
	async fn approved(&mut self, _: &NodeAddress, _: &EntityRef) -> Result<bool> {
		Ok(self.approved)
	}
	async fn write_carried(
		&mut self,
		_: &NodeAddress,
		source: &Source,
		entry: &Entry,
		scope: ApprovalScope,
	) -> Result<()> {
		self.writes
			.push((source.definition.version.clone(), entry.version.clone()));
		self.scopes.push(scope);
		Ok(())
	}
}

#[tokio::test]
async fn a_backport_carries_only_from_the_highest_lower_opted_in_version() {
	let mut fixture = Fixture {
		sources: vec![
			source("2.1.0", true),
			source("2.0.0", true),
			source("2.0.1-alpha", false),
		],
		..Default::default()
	};
	let result = carry(&mut fixture, &model("2.0.1", "provider"))
		.await
		.unwrap();
	assert_eq!(fixture.writes, vec![("2.0.0".into(), "2.0.1".into())]);
	assert!(result[0].carried);
	assert_eq!(result[0].source_version.as_deref(), Some("2.0.0"));
}

#[tokio::test]
async fn compatible_builtin_approval_decider_version_can_carry() {
	// The compaction Decider was removed by #853; the approval Decider is the
	// remaining built-in Decider.
	let next = crate::registry::system::approval_decider().unwrap();
	let mut previous = next.clone();
	previous.version = "0.9.0".into();
	let mut fixture = Fixture {
		sources: vec![Source {
			definition: previous,
			enabled: true,
			carry_over: true,
			scope: ApprovalScope::Deployment,
			approver: "alice".into(),
		}],
		..Default::default()
	};
	let result = carry(&mut fixture, &next).await.unwrap();
	assert!(result[0].carried);
	assert_eq!(fixture.writes, [("0.9.0".into(), next.version)]);
}

#[tokio::test]
async fn a_changed_consent_surface_is_reported_without_writing_an_approval() {
	let mut fixture = Fixture {
		sources: vec![source("1.0.0", true)],
		..Default::default()
	};
	let result = carry(&mut fixture, &model("1.1.0", "other")).await.unwrap();
	assert!(fixture.writes.is_empty());
	assert!(!result[0].carried);
	assert_eq!(result[0].changed, ["provider"]);
}

#[tokio::test]
async fn an_unapproved_dependency_prevents_agent_carry() {
	let agent = |version: &str| -> Entry {
		serde_json::from_value(json!({"id":"writer","version":version,"kind":"agent","name":{},"description":{},"config":{"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"help","remove_default":[]}})).unwrap()
	};
	let mut fixture = Fixture {
		sources: vec![Source {
			definition: agent("1.0.0"),
			enabled: true,
			carry_over: true,
			scope: ApprovalScope::Node,
			approver: "alice".into(),
		}],
		..Default::default()
	};
	let result = carry(&mut fixture, &agent("1.1.0")).await.unwrap();
	assert!(fixture.writes.is_empty());
	assert_eq!(
		result[0].unapproved_dependencies,
		[EntityRef {
			id: "model".into(),
			version: "1.0.0".into()
		}]
	);
}

#[tokio::test]
async fn invalid_consent_surface_reports_failure_without_writing() {
	let mut fixture = Fixture {
		sources: vec![source("1.0.0", true)],
		..Default::default()
	};
	let mut entry = model("1.1.0", "provider");
	entry.config = json!({"provider": "incomplete"});
	let result = carry(&mut fixture, &entry).await.unwrap();
	assert!(!result[0].carried);
	assert!(result[0].changed[0].contains("consent comparison failed"));
	assert!(fixture.writes.is_empty());
}

#[tokio::test]
async fn a_carry_that_fails_on_one_node_writes_node_scoped_rows_elsewhere() {
	let deployment = |version: &str, provider: &str| Source {
		definition: model(version, provider),
		enabled: true,
		carry_over: true,
		scope: ApprovalScope::Deployment,
		approver: "alice".into(),
	};
	let other: NodeAddress = "aidash://local/calm-heron-p4t7w2zb".parse().unwrap();
	let mut fixture = Fixture {
		sources: vec![deployment("1.0.0", "provider")],
		approved: true,
		other_nodes: vec![(other, vec![deployment("1.0.0", "other")])],
		..Default::default()
	};
	let result = carry(&mut fixture, &model("1.1.0", "provider"))
		.await
		.unwrap();
	assert!(result[0].carried);
	assert!(!result[1].carried);
	assert_eq!(fixture.scopes, [ApprovalScope::Node]);

	let mut fixture = Fixture {
		sources: vec![deployment("1.0.0", "provider")],
		approved: true,
		..Default::default()
	};
	carry(&mut fixture, &model("1.1.0", "provider"))
		.await
		.unwrap();
	assert_eq!(fixture.scopes, [ApprovalScope::Deployment]);
}
