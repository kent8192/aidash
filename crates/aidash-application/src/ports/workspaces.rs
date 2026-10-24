//! One caller-owned authorization transaction covers conversation admission.
use crate::Result;
use aidash_domain::{
	Conversation, NewTask, Task, Workspace,
	federation::Delegation,
	nodes::NodeName,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use uuid::Uuid;

/// The owner must finish the scope once: success commits writes and audit;
/// failure rolls back writes and persists denial audit under the existing policy.
#[async_trait]
pub trait ConversationScope: Send {
	async fn create_workspace(
		&mut self,
		title: &str,
		goal: &str,
		node: Option<&NodeName>,
	) -> Result<Workspace>;
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

pub mod mutations;
