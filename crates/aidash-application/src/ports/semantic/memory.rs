//! Agent memory and persisted read dependencies retain the caller's authority scope.
use crate::Result;
use aidash_domain::semantic::mutations::Entry;
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait SemanticMemoryReadSession: Send {
	async fn native_reads_visible(&mut self, _run: Uuid) -> Result<bool> {
		Ok(true)
	}
	async fn permits(&mut self, entry: &Entry, action: &str) -> Result<bool>;
	async fn source(
		&mut self,
		workspace: Uuid,
		source: &aidash_domain::semantic::Source,
	) -> Result<Option<String>>;
	/// Preserve ascending entry/revision order before checking authority and source bytes.
	async fn dependencies(&mut self, run: Uuid) -> Result<Vec<(Uuid, i64)>>;
	/// Missing rows preserve the adapter contract: absence denies native disclosure; SQL errors remain errors.
	async fn entry(&mut self, id: Uuid) -> Result<Option<Entry>>;
	/// A nullable persisted digest remains distinct from a missing database row.
	async fn point_digest(&mut self, point: Uuid) -> Result<Option<String>>;
}
