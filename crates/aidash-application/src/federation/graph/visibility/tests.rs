use super::*;
use aidash_domain::{Conversation, registry::Entry};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::json;
use uuid::Uuid;

struct Scope {
	operator: bool,
	permitted: bool,
	trace: Vec<String>,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		operator: false,
		permitted: false,
		trace: vec![],
	}
}
#[fixture]
fn conversation() -> Candidate {
	Candidate::Conversation(Conversation {
		created_by: "subject".into(),
		id: Uuid::new_v4(),
		workspace_id: Uuid::new_v4(),
		target: "agent@1".into(),
		target_kind: "agent".into(),
		created_at: chrono::Utc::now(),
	})
}
#[async_trait]
impl GraphVisibility for Scope {
	fn operator(&self) -> bool {
		self.operator
	}
	async fn operator_workspace(&mut self, _: Uuid) -> Result<bool> {
		self.trace.push("operator-workspace".into());
		Ok(self.permitted)
	}
	async fn registry(&mut self, _: &Entry, action: &str) -> Result<bool> {
		self.trace.push(action.into());
		Ok(self.permitted)
	}
	async fn workspace(&mut self, _: Uuid, action: &str) -> Result<bool> {
		self.trace.push(action.into());
		Ok(self.permitted)
	}
	async fn record(&mut self, _: &Candidate) -> Result<bool> {
		self.trace.push("record".into());
		Ok(self.permitted)
	}
	async fn conversation(&mut self, _: &Conversation, action: &str) -> Result<bool> {
		self.trace.push(action.into());
		Ok(self.permitted)
	}
}

#[rstest]
#[tokio::test]
async fn denied_workspace_cannot_disclose_conversation_resources(
	mut scope: Scope,
	conversation: Candidate,
) {
	assert_eq!(visible(&mut scope, &conversation).await.unwrap(), false);
	assert_eq!(scope.trace, ["workspace.read"]);
}

#[rstest]
#[tokio::test]
async fn subject_requires_workspace_before_conversation_permission(
	mut scope: Scope,
	conversation: Candidate,
) {
	scope.permitted = true;
	assert_eq!(visible(&mut scope, &conversation).await.unwrap(), true);
	assert_eq!(scope.trace, ["workspace.read", "conversation.read"]);
}

#[rstest]
#[tokio::test]
async fn operator_uses_its_granted_tenant_workspace_scope(
	mut scope: Scope,
	conversation: Candidate,
) {
	scope.operator = true;
	assert_eq!(visible(&mut scope, &conversation).await.unwrap(), false);
	assert_eq!(scope.trace, ["operator-workspace"]);
}

#[rstest]
#[case(false, false, vec!["registry.read"])]
#[case(true, true, vec![])]
#[tokio::test]
async fn catalog_visibility_retains_viewer_specific_permission(
	mut scope: Scope,
	#[case] operator: bool,
	#[case] allowed: bool,
	#[case] calls: Vec<&str>,
) {
	scope.operator = operator;
	let entry: Entry = serde_json::from_value(
		json!({"id":"agent","version":"1","kind":"agent","name":{},"description":{}}),
	)
	.unwrap();
	assert_eq!(
		visible(&mut scope, &Candidate::Registry(entry))
			.await
			.unwrap(),
		allowed
	);
	assert_eq!(scope.trace, calls);
}
