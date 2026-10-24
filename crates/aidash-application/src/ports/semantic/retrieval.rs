//! A retrieval scope keeps the original credential, index, entry, and source locks.
use crate::Result;
use aidash_domain::semantic::{
	EmbeddingConfig, Source,
	mutations::{Entry, Index},
};
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait SemanticRetrievalSession: Send {
	async fn workspace(&mut self, workspace: Uuid, action: &str) -> Result<()>;
	/// Retain the same shared index lock through candidate and delivery checks.
	async fn index(&mut self, workspace: Uuid) -> Result<Index>;
	/// Lock nondeleted entries in ascending ID order before any provider effect.
	/// Read optional auto-context configuration with the same shared index lock.
	async fn configured(&mut self, workspace: Uuid) -> Result<Option<Index>>;
	async fn candidates(&mut self, workspace: Uuid) -> Result<Vec<Entry>>;
	async fn permits(&mut self, entry: &Entry, action: &str) -> Result<bool>;
	async fn source(&mut self, workspace: Uuid, source: &Source) -> Result<Option<String>>;
	/// A missing or retired point has the same empty digest as the old query.
	async fn digest(&mut self, point: Uuid) -> Result<String>;
	/// Reserve, invoke, and settle under this same restored authority scope.
	async fn embed(
		&mut self,
		workspace: Uuid,
		config: &EmbeddingConfig,
		text: &str,
		run: Option<Uuid>,
	) -> Result<Vec<f32>>;
}
