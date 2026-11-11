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
	approved: bool,
}
#[async_trait]
impl CarryScope for Fixture {
	async fn nodes(&mut self) -> Result<Vec<NodeAddress>> {
		Ok(vec![node()])
	}
	async fn sources(&mut self, _: &NodeAddress, _: &str) -> Result<Vec<Source>> {
		Ok(self.sources.clone())
	}
	async fn approved(&mut self, _: &NodeAddress, _: &EntityRef) -> Result<bool> {
		Ok(self.approved)
	}
	async fn write_carried(
		&mut self,
		_: &NodeAddress,
		source: &Source,
		entry: &Entry,
	) -> Result<()> {
		self.writes
			.push((source.definition.version.clone(), entry.version.clone()));
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
		serde_json::from_value(json!({"id":"writer","version":version,"kind":"agent","name":{},"description":{},"config":{"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"help","remove_default":["compaction"]}})).unwrap()
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
