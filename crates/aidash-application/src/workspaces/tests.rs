use super::*;
use crate::Error;
use aidash_domain::{
	Conversation, Task, TaskStatus, Workspace, federation::Delegation, registry::Entry,
};
use async_trait::async_trait;
use uuid::Uuid;

struct Fixture {
	target: Entry,
	operations: Vec<String>,
	denied: Option<&'static str>,
	delegated_agent: Option<EntityRef>,
	task: Task,
}
impl Fixture {
	fn new(kind: &str) -> Self {
		Self {
            target: serde_json::from_value(json!({
                "id":"target", "version":"1", "kind":kind, "name":{"en":"Target"}, "description":{"en":"Fixture"},
                "config":{"coordinator":{"id":"coordinator", "version":"2"}},
            })).unwrap(),
            operations: vec![], denied: None, delegated_agent: None,
            task: Task {
                id: Uuid::from_u128(3), workspace_id: Uuid::from_u128(1), title:"Title".into(), description:"Goal".into(),
                status:TaskStatus::Open, requirements:json!({}), owner:None, created_by:"subject:fixture".into(),
                dependencies:vec![], parent_id:None, revision:0, created_at:chrono::Utc::now(),
            },
        }
	}
	fn require(&mut self, action: &str) -> Result<()> {
		self.operations.push(action.into());
		if self.denied == Some(action) {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
}
#[async_trait]
impl ConversationScope for Fixture {
	async fn create_workspace(&mut self, title: &str, goal: &str) -> Result<Workspace> {
		self.operations.push("workspace.create".into());
		Ok(Workspace {
			id: Uuid::from_u128(1),
			title: title.into(),
			goal: goal.into(),
			state: json!({}),
			revision: 0,
			created_at: chrono::Utc::now(),
		})
	}
	async fn workspace_context(&mut self, _workspace: Uuid) -> Result<()> {
		self.operations.push("context".into());
		Ok(())
	}
	async fn require_workspace(&mut self, action: &str) -> Result<()> {
		self.require(action)
	}
	async fn registry_entry(&mut self, _reference: &EntityRef, action: &str) -> Result<Entry> {
		self.require(action)?;
		Ok(self.target.clone())
	}
	async fn create_conversation(
		&mut self,
		workspace_id: Uuid,
		target: &EntityRef,
		target_kind: &str,
	) -> Result<Conversation> {
		self.operations.push("insert.conversation".into());
		Ok(Conversation {
			id: Uuid::from_u128(2),
			workspace_id,
			target: format!("{}@{}", target.id, target.version),
			target_kind: target_kind.into(),
			created_at: chrono::Utc::now(),
			created_by: "subject:fixture".into(),
		})
	}
	async fn require_conversation(
		&mut self,
		_conversation: &Conversation,
		action: &str,
	) -> Result<()> {
		self.require(action)
	}
	async fn conversation_event(&mut self, _conversation: &Conversation) -> Result<()> {
		self.operations.push("event.conversation".into());
		Ok(())
	}
	async fn message(&mut self, _workspace: Uuid, content: &str) -> Result<()> {
		assert_eq!(content, "Goal");
		self.operations.push("insert.message".into());
		Ok(())
	}
	async fn create_task(&mut self, workspace: Uuid, input: &NewTask) -> Result<Task> {
		assert_eq!(workspace, self.task.workspace_id);
		assert_eq!(input.title, "Title");
		assert_eq!(input.description, "Goal");
		assert_eq!(input.requirements, json!({}));
		assert!(input.dependencies.is_empty());
		assert!(input.parent_id.is_none());
		self.operations.push("insert.task".into());
		Ok(self.task.clone())
	}
	async fn require_task(&mut self, _task: &Task, action: &str) -> Result<()> {
		self.require(action)
	}
	async fn bind_task(&mut self, task: Uuid, conversation: Uuid) -> Result<()> {
		assert_eq!((task, conversation), (self.task.id, Uuid::from_u128(2)));
		self.operations.push("bind.session".into());
		Ok(())
	}
	async fn delegate(&mut self, task: Uuid, agent: &EntityRef) -> Result<Delegation> {
		self.operations.push("delegate".into());
		self.delegated_agent = Some(agent.clone());
		self.task.revision = 7;
		self.task.status = TaskStatus::Claimed;
		Ok(Delegation {
			task_id: task,
			node_id: "aidash://fixture".into(),
			agent_id: agent.id.clone(),
			agent_version: agent.version.clone(),
			delivered: false,
		})
	}
	async fn task(&mut self, task: Uuid) -> Result<Task> {
		assert_eq!(task, self.task.id);
		self.operations.push("reload.task".into());
		Ok(self.task.clone())
	}
}
fn request<'a>(reference: &'a EntityRef, kind: &'a str) -> ConversationRequest<'a> {
	ConversationRequest {
		title: "Title",
		goal: "Goal",
		target: reference,
		target_kind: kind,
	}
}

#[rstest::rstest]
#[case("agent", "target", "1")]
#[case("cluster", "coordinator", "2")]
#[tokio::test]
async fn authorized_admission_binds_the_session_and_returns_the_delegated_revision(
	#[case] kind: &str,
	#[case] executor: &str,
	#[case] version: &str,
) {
	// Arrange / Act: the same borrowed scope receives all policy and write operations.
	let reference = EntityRef {
		id: "target".into(),
		version: "1".into(),
	};
	let mut scope = Fixture::new(kind);
	let result = conversation(&mut scope, request(&reference, kind))
		.await
		.unwrap();
	// Assert: the conversation retains its cluster identity while execution pins the coordinator.
	assert_eq!(result.conversation.target, "target@1");
	assert_eq!(result.conversation.created_by, "subject:fixture");
	assert_eq!(result.task.revision, 7);
	assert_eq!(result.task.status, TaskStatus::Claimed);
	assert_eq!(result.delegation.agent_id, executor);
	assert_eq!(result.delegation.agent_version, version);
	let before = |a: &str, b: &str| {
		scope.operations.iter().position(|op| op == a).unwrap()
			< scope.operations.iter().position(|op| op == b).unwrap()
	};
	assert!(before("conversation.read", "event.conversation"));
	assert!(before("message.create", "insert.message"));
	assert!(before("task.create", "insert.task"));
	assert!(before("task.read", "bind.session"));
	assert!(before("bind.session", "delegate"));
	assert!(before("delegate", "reload.task"));
	assert_eq!(
		scope
			.operations
			.iter()
			.filter(|op| op.as_str() == "cluster.execute")
			.count(),
		usize::from(kind == "cluster")
	);
}

#[rstest::rstest]
#[case("conversation.read", "event.conversation")]
#[case("message.create", "insert.message")]
#[case("task.read", "bind.session")]
#[tokio::test]
async fn denial_stops_before_the_protected_effect_and_delegation(
	#[case] denied: &'static str,
	#[case] effect: &str,
) {
	let reference = EntityRef {
		id: "target".into(),
		version: "1".into(),
	};
	let mut scope = Fixture::new("agent");
	scope.denied = Some(denied);
	let result = conversation(&mut scope, request(&reference, "agent")).await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert!(!scope.operations.iter().any(|op| op == effect));
	assert!(!scope.operations.iter().any(|op| op == "delegate"));
}

#[rstest::rstest]
#[tokio::test]
async fn malformed_cluster_coordinator_cannot_publish_a_conversation_or_task() {
	let reference = EntityRef {
		id: "target".into(),
		version: "1".into(),
	};
	let mut scope = Fixture::new("cluster");
	scope.target.config = json!({});
	let result = conversation(&mut scope, request(&reference, "cluster")).await;
	assert!(
		matches!(result, Err(Error::Domain(aidash_domain::Error::Invalid(ref message))) if message=="cluster requires an explicit coordinator agent reference")
	);
	assert!(scope.operations.iter().any(|op| op == "cluster.execute"));
	assert!(
		!scope
			.operations
			.iter()
			.any(|op| op == "insert.conversation" || op == "insert.task")
	);
}
