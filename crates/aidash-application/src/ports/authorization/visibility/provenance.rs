//! Provenance readers retain row scope and the current authority across the whole graph.
use crate::{
	Result,
	ports::{
		authorization::visibility::resources::ResourceVisibilityScope,
		federation::registry_reads::RemoteReads,
	},
};
use aidash_domain::{
	Artifact, Conversation, Message, RunMetadata, Task, generation::requests::Request,
	registry::EntityRef, semantic::Failure,
};
use async_trait::async_trait;
use uuid::Uuid;
pub type RecordedRead = (Uuid, String, Uuid);
#[async_trait]
pub trait ReadProvenanceScope: ResourceVisibilityScope {
	/// Open the Subnode Workspaces that `run` created or works in while its reads
	/// are checked. An inherited authority otherwise reaches only its own
	/// Workspace, and a Run's reads may lie in those (ADR 0110). Returns whether
	/// anything opened; a caller that got `true` calls [`Self::close_run_reads`]
	/// after that Run.
	async fn open_run_reads(&mut self, _run: Uuid) -> Result<bool> {
		Ok(false)
	}
	fn close_run_reads(&mut self) {}
	async fn registry_entries(&mut self, run: Uuid) -> Result<Vec<EntityRef>>;
	/// Whether the reader may read the definition `run` read: `registry.read` alone. A Catalog
	/// Approval decides use, never who may read what a Run produced (#769).
	async fn catalog_read(&mut self, run: Uuid, reference: &EntityRef) -> Result<()>;
	async fn remote_reads(&mut self, run: Uuid) -> Result<RemoteReads>;
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
	/// Whether this is the first time the walk in progress reaches `run`. A walk
	/// passes only if every Run it reaches passes, so one look at each is enough:
	/// a Run reached again either passed or will fail the walk anyway.
	fn first_visit(&mut self, _run: Uuid) -> bool {
		true
	}
	/// Load what these reads will ask for in advance, so a walk's statement count
	/// does not grow with the reads. A reader that only reads rows one at a time
	/// has nothing to do here, and the checks are the same either way.
	async fn prepare_sources(&mut self, _sources: &[RecordedRead]) -> Result<()> {
		Ok(())
	}
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
	/// Resolve an extant attachment to its parent message for current-authority checks.
	async fn source_attachment_message(
		&mut self,
		id: Uuid,
		workspace: Uuid,
	) -> Result<Option<Message>>;
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
