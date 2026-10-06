//! One caller-owned authorization transaction covers conversation admission.
use crate::Result;
use aidash_domain::{
	Conversation, NewTask, Task, Workspace,
	federation::Delegation,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use uuid::Uuid;

/// The owner must finish the scope once: success commits writes and audit;
/// failure rolls back writes and persists denial audit under the existing policy.
#[async_trait]
pub trait ConversationScope: Send {
	async fn create_workspace(&mut self, title: &str, goal: &str) -> Result<Workspace>;
	async fn workspace_context(&mut self, workspace: Uuid) -> Result<()>;
	async fn require_workspace(&mut self, action: &str) -> Result<()>;
	async fn registry_entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry>;
	async fn create_conversation(
		&mut self,
		workspace: Uuid,
		target: &EntityRef,
		target_kind: &str,
	) -> Result<Conversation>;
	async fn require_conversation(
		&mut self,
		conversation: &Conversation,
		action: &str,
	) -> Result<()>;
	async fn conversation_event(&mut self, conversation: &Conversation) -> Result<()>;
	async fn message(&mut self, workspace: Uuid, content: &str) -> Result<()>;
	async fn create_task(&mut self, workspace: Uuid, input: &NewTask) -> Result<Task>;
	async fn require_task(&mut self, task: &Task, action: &str) -> Result<()>;
	async fn bind_task(&mut self, task: Uuid, conversation: Uuid) -> Result<()>;
	async fn delegate(&mut self, task: Uuid, agent: &EntityRef) -> Result<Delegation>;
	async fn task(&mut self, task: Uuid) -> Result<Task>;
}

/// A fully privileged operator has already authenticated at the outer boundary.
/// Preflight definition checks happen before this store opens its transaction.
#[async_trait]
pub trait OperatorConversations: Send + Sync {
	async fn definition(&self, reference: &EntityRef) -> Result<Entry>;
	async fn require_legacy_agent(&self, reference: &EntityRef) -> Result<()>;
	async fn begin(&self) -> Result<Box<dyn OperatorConversationTransaction>>;
	async fn deliver(&self, delegation: &Delegation) -> Result<()>;
}
/// All initial records, event/outbox effects, and delegation share this scope.
#[async_trait]
pub trait OperatorConversationTransaction: Send {
	async fn create_workspace(&mut self, title: &str, goal: &str) -> Result<Workspace>;
	async fn create_conversation(
		&mut self,
		workspace: Uuid,
		target: &EntityRef,
		target_kind: &str,
	) -> Result<Conversation>;
	async fn conversation_event(&mut self, conversation: &Conversation) -> Result<()>;
	async fn message(&mut self, workspace: Uuid, content: &str) -> Result<()>;
	async fn create_task(&mut self, workspace: Uuid, input: &NewTask) -> Result<Task>;
	async fn delegate(&mut self, task: &Task, agent: &EntityRef) -> Result<(Task, Delegation)>;
	async fn commit(self: Box<Self>) -> Result<()>;
}

pub mod mutations;
