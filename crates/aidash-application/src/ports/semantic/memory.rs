//! Agent memory and persisted read dependencies retain the caller's authority scope.
use super::mutations::SemanticEntriesSession;
use crate::Result;
use aidash_domain::{Run, semantic::mutations::Entry};
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait SemanticMemoryWriteSession: SemanticEntriesSession {
	async fn configured(&mut self, workspace: Uuid) -> Result<bool>;
	async fn revision(&mut self, workspace: Uuid, key: &str) -> Result<Option<i64>>;
	async fn bind_memory(&mut self, entry: &Entry, run: &Run) -> Result<()>;
	/// Persist ordinary memory after the optional semantic write on this same scope.
	async fn persist_memory(&mut self, run: &Run, data: &serde_json::Value) -> Result<()>;
}

#[async_trait]
pub trait SemanticMemoryReadSession: Send {
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
