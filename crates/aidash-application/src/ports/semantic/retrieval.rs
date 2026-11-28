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
pub trait SemanticRetrievalSession:
	Send + crate::ports::authorization::visibility::RowOutcome
{
	async fn thread(&mut self, thread: Uuid, action: &str) -> Result<()>;
	/// Retain the same shared index lock through candidate and delivery checks.
	async fn index(&mut self, thread: Uuid) -> Result<Index>;
	/// Lock nondeleted entries in ascending ID order before any provider effect.
	/// Read optional auto-context configuration with the same shared index lock.
	async fn configured(&mut self, thread: Uuid) -> Result<Option<Index>>;
	async fn candidates(&mut self, thread: Uuid) -> Result<Vec<Entry>>;
	async fn permits(&mut self, entry: &Entry, action: &str) -> Result<bool>;
	async fn source(&mut self, thread: Uuid, source: &Source) -> Result<Option<String>>;
	/// Return only metadata for the currently authorized File version, never its bytes.
	async fn image_pointer(&mut self, _thread: Uuid, _source: &Source) -> Result<Option<Value>> {
		Ok(None)
	}
	/// Whether the query's Run itself selected this attachment, with these
	/// bytes, for its own inference.
	async fn attachment_seen(
		&mut self,
		_query: &aidash_domain::semantic::retrieval::AttachmentImageQuery,
	) -> Result<bool> {
		Ok(false)
	}
	/// Read a selected attachment under its Run's current message authority.
	async fn query_attachment(
		&mut self,
		_thread: Uuid,
		_query: &aidash_domain::semantic::retrieval::AttachmentImageQuery,
		_max_bytes: usize,
	) -> Result<super::IndexImage> {
		Err(crate::Error::Forbidden)
	}
	async fn embed_image(
		&mut self,
		_thread: Uuid,
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
		thread: Uuid,
		config: &EmbeddingConfig,
		text: &str,
		run: Option<Uuid>,
	) -> Result<Vec<f32>>;
}
