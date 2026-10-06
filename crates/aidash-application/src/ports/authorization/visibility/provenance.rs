//! Provenance readers retain row scope and the current authority across the whole graph.
use crate::{Result, ports::authorization::visibility::resources::ResourceVisibilityScope};
use aidash_domain::{
	Artifact, Conversation, Message, RunMetadata, Task, generation::requests::Request,
	registry::EntityRef,
};
use async_trait::async_trait;
use uuid::Uuid;
pub type RecordedRead = (Uuid, String, Uuid);
#[async_trait]
pub trait ReadProvenanceScope: ResourceVisibilityScope {
	async fn registry_entries(&mut self, run: Uuid) -> Result<Vec<EntityRef>>;
	async fn catalog_read(&mut self, reference: &EntityRef) -> Result<()>;
	async fn remote_reads(&mut self, run: Uuid) -> Result<bool>;
	async fn semantic_reads(&mut self, run: Uuid) -> Result<bool>;
	/// The native local reader has no received-semantic journal; its foreign reader owns that check.
	async fn received_semantic(&mut self, _: Uuid) -> Result<bool> {
		Ok(true)
	}
	async fn sources(&mut self, run: Uuid) -> Result<Vec<RecordedRead>>;
	async fn source_task(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Task>>;
	async fn source_artifact(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Artifact>>;
	async fn source_message(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Message>>;
	async fn source_run(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<RunMetadata>>;
	async fn source_conversation(
		&mut self,
		id: Uuid,
		workspace: Uuid,
	) -> Result<Option<Conversation>>;
	async fn source_generation(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Request>>;
	async fn run_base_visible(&mut self, run: &RunMetadata) -> Result<bool>;
	async fn generation_visible(&mut self, job: &Request) -> Result<bool>;
}

/// Producers retain the repository's ascending UUID order and exact resource scope.
#[async_trait]
pub trait LocalOutputScope: Send {
	async fn producers(&mut self, workspace: Uuid, kind: &str, id: Uuid) -> Result<Vec<Uuid>>;
	async fn producer_reads_visible(&mut self, run: Uuid) -> Result<bool>;
}
#[async_trait]
pub trait OutputScope: LocalOutputScope {
	async fn remote_grants(&mut self, workspace: Uuid, kind: &str, id: Uuid) -> Result<Vec<Uuid>>;
	async fn grant_visible(&mut self, grant: Uuid) -> Result<bool>;
}
