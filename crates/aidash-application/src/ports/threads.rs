//! One caller-owned authorization transaction covers conversation admission.
use crate::Result;
use aidash_domain::{
	Conversation, NewTask, Task, Thread,
	federation::Delegation,
	nodes::NodeName,
	registry::{EntityRef, Entry},
	semantic::{EmbeddingConfig, indexing::IndexingSpec},
};
use async_trait::async_trait;
use uuid::Uuid;

/// The owner must finish the scope once: success commits writes and audit;
/// failure rolls back writes and persists denial audit under the existing policy.
#[async_trait]
pub trait ConversationScope: Send {
	async fn create_thread(
		&mut self,
		title: &str,
		node: Option<&NodeName>,
		scratch_space_id: Option<Uuid>,
	) -> Result<Thread>;
	async fn thread_context(&mut self, thread: Uuid) -> Result<()>;
	async fn require_thread(&mut self, action: &str) -> Result<()>;
	async fn registry_entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry>;
	async fn select_scratch_space(
		&mut self,
		thread: &Thread,
		name: &str,
		requested: Option<Uuid>,
	) -> Result<Uuid>;
	async fn create_conversation(
		&mut self,
		thread: Uuid,
		scratch_space_id: Uuid,
		target: &EntityRef,
		target_kind: &str,
	) -> Result<Conversation>;
	async fn require_conversation(
		&mut self,
		conversation: &Conversation,
		action: &str,
	) -> Result<()>;
	async fn conversation_event(&mut self, conversation: &Conversation) -> Result<()>;
	async fn message(&mut self, thread: Uuid, content: &str) -> Result<()>;
	async fn create_task(&mut self, thread: Uuid, input: &NewTask) -> Result<Task>;
	async fn require_task(&mut self, task: &Task, action: &str) -> Result<()>;
	/// The memory provider `agent` binds and the embedding its policy pins, or
	/// `None` when the agent binds no memory.
	async fn memory_embedding(&mut self, agent: &EntityRef) -> Result<Option<MemoryEmbedding>>;
	/// Create the conversation's new Thread's semantic index for `agent`'s
	/// memory (#647), recorded as an event of the conversation.
	async fn create_memory_index(
		&mut self,
		conversation: &Conversation,
		agent: &EntityRef,
		provider: &EntityRef,
		spec: &IndexingSpec,
	) -> Result<()>;
	async fn delegate(&mut self, task: Uuid, agent: &EntityRef) -> Result<Delegation>;
	async fn task(&mut self, task: Uuid) -> Result<Task>;
}

/// A memory provider and the embedding its policy pins.
#[derive(Debug, Clone)]
pub struct MemoryEmbedding {
	pub provider: EntityRef,
	pub embedding: EmbeddingConfig,
}

pub mod mutations;
