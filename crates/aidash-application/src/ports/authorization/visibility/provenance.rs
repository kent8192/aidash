//! Provenance readers retain row scope and the current authority across the whole graph.
use crate::{Result, ports::authorization::visibility::resources::ResourceVisibilityScope};
use aidash_domain::{
	Artifact, Conversation, Message, RunMetadata, Task, generation::requests::Request,
	registry::EntityRef, semantic::Failure,
};
use async_trait::async_trait;
use uuid::Uuid;
pub type RecordedRead = (Uuid, String, Uuid);
#[async_trait]
pub trait ReadProvenanceScope: ResourceVisibilityScope {
	async fn registry_entries(&mut self, run: Uuid) -> Result<Vec<EntityRef>>;
	async fn catalog_read(&mut self, run: Uuid, reference: &EntityRef) -> Result<()>;
	async fn remote_reads(&mut self, run: Uuid) -> Result<bool>;
	async fn semantic_reads(&mut self, run: Uuid) -> Result<bool>;
	async fn semantic_invalidity(
		&mut self,
		run: Uuid,
		_pending: &mut Vec<Uuid>,
	) -> Result<Option<Failure>> {
		Ok((!self.semantic_reads(run).await?).then_some(Failure::Authority))
	}
	/// The native local reader has no received-semantic journal; its foreign reader owns that check.
	async fn received_semantic(&mut self, _: Uuid) -> Result<bool> {
		Ok(true)
	}
	async fn sources(&mut self, run: Uuid) -> Result<Vec<RecordedRead>>;
	/// Check remote output grants and return local producers for classified traversal.
	/// `None` means the output's remote provenance is not visible.
	async fn output_producers(
		&mut self,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<Option<Vec<Uuid>>>;
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
	/// A recorded Run dependency may have no private memory reads to protect.
	async fn run_dependency_visible(&mut self, run: &RunMetadata) -> Result<bool> {
		self.run_base_visible(run).await
	}
	/// Apply dependency resource policies while deferring Task producer journals.
	async fn classified_run_dependency_visible(
		&mut self,
		run: &RunMetadata,
		pending: &mut Vec<Uuid>,
	) -> Result<bool>;
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
