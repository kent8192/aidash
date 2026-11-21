use super::*;
use aidash_domain::{Conversation, registry::Entry};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::json;
use uuid::Uuid;

struct Scope {
	permitted: bool,
	trace: Vec<String>,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		permitted: false,
		trace: vec![],
	}
}
#[fixture]
fn conversation() -> Candidate {
	Candidate::Conversation(Conversation {
		scratch_space_id: uuid::Uuid::nil(),
		created_by: "subject".into(),
		id: Uuid::new_v4(),
		thread_id: Uuid::new_v4(),
		target: "agent@1".into(),
		target_kind: "agent".into(),
		created_at: chrono::Utc::now(),
	})
}
#[async_trait]
impl GraphVisibility for Scope {
	async fn registry(&mut self, _: &Entry, action: &str) -> Result<bool> {
		self.trace.push(action.into());
		Ok(self.permitted)
	}
	async fn thread(&mut self, _: Uuid, action: &str) -> Result<bool> {
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
async fn denied_thread_cannot_disclose_conversation_resources(
	mut scope: Scope,
	conversation: Candidate,
) {
	assert!(!visible(&mut scope, &conversation).await.unwrap());
	assert_eq!(scope.trace, ["thread.read"]);
}

#[rstest]
#[tokio::test]
async fn subject_requires_thread_before_conversation_permission(
	mut scope: Scope,
	conversation: Candidate,
) {
	scope.permitted = true;
	assert!(visible(&mut scope, &conversation).await.unwrap());
	assert_eq!(scope.trace, ["thread.read", "conversation.read"]);
}

#[rstest]
#[tokio::test]
async fn catalog_visibility_retains_subject_specific_permission(mut scope: Scope) {
	let entry: Entry = serde_json::from_value(
		json!({"id":"agent","version":"1","kind":"agent","name":{},"description":{}}),
	)
	.unwrap();
	assert!(
		!visible(&mut scope, &Candidate::Registry(Box::new(entry)))
			.await
			.unwrap()
	);
	assert_eq!(scope.trace, ["registry.read"]);
}
