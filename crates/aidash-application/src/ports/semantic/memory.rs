//! Agent memory and persisted read dependencies retain the caller's authority scope.
use crate::Result;
use aidash_domain::semantic::Failure;
use aidash_domain::semantic::mutations::Entry;
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait SemanticMemoryReadSession: Send {
	async fn native_reads_visible(&mut self, _run: Uuid) -> Result<bool> {
		Ok(true)
	}
	async fn native_invalidity(&mut self, run: Uuid) -> Result<Option<Failure>> {
		Ok((!self.native_reads_visible(run).await?).then_some(Failure::Authority))
	}
	async fn permits(&mut self, entry: &Entry, action: &str) -> Result<bool>;
	async fn entry_authorized(&mut self, entry: &Entry) -> Result<bool> {
		self.permits(entry, "semantic.read").await
	}
	async fn source(
		&mut self,
		workspace: Uuid,
		source: &aidash_domain::semantic::Source,
	) -> Result<Option<String>>;
	/// Resolve an artifact or message while adding its producers to the shared classified walk.
	async fn classified_output_source(
		&mut self,
		workspace: Uuid,
		source: &aidash_domain::semantic::Source,
		pending: &mut Vec<Uuid>,
	) -> Result<Option<String>>;
	/// Whether the image a recorded read pointed at is still there for this
	/// reader: its File version in an Area they can read, or its attachment in
	/// its message. Refuses unless an adapter checks.
	async fn image_visible(
		&mut self,
		_: Uuid,
		_: &aidash_domain::semantic::Source,
	) -> Result<bool> {
		Ok(false)
	}
	/// True only when the stored source is provably missing or withdrawn.
	async fn source_invalidated(
		&mut self,
		_: Uuid,
		_: &aidash_domain::semantic::Source,
	) -> Result<bool> {
		Ok(false)
	}
	/// Preserve ascending entry/revision order before checking authority and source bytes.
	async fn dependencies(&mut self, run: Uuid) -> Result<Vec<(Uuid, i64)>>;
	/// Missing rows preserve the adapter contract: absence denies native disclosure; SQL errors remain errors.
	async fn entry(&mut self, id: Uuid) -> Result<Option<Entry>>;
	/// A nullable persisted digest remains distinct from a missing database row.
	async fn point_digest(&mut self, point: Uuid) -> Result<Option<String>>;
}
