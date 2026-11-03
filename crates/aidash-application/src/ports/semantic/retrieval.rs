//! A retrieval scope keeps the original credential, index, entry, and source locks.
use crate::Result;
use aidash_domain::semantic::{
	EmbeddingConfig, Source,
	mutations::{Entry, Index},
};
use async_trait::async_trait;
use serde_json::Value;
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
	/// Return only metadata for the currently authorized File version, never its bytes.
	async fn image_pointer(&mut self, _workspace: Uuid, _source: &Source) -> Result<Option<Value>> {
		Ok(None)
	}
	/// Resolve an image only under the named Run's current Working Area authority.
	async fn query_image(
		&mut self,
		_workspace: Uuid,
		_query: &aidash_domain::semantic::retrieval::ImageQuery,
		_max_bytes: usize,
	) -> Result<super::IndexImage> {
		Err(crate::Error::Forbidden)
	}
	async fn embed_image(
		&mut self,
		_workspace: Uuid,
		_config: &EmbeddingConfig,
		_image: &super::IndexImage,
		_run: Option<Uuid>,
	) -> Result<Vec<f32>> {
		Err(crate::Error::Forbidden)
	}
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
