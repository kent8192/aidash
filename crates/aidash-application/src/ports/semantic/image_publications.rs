//! A publication and its quota changes share one durable transaction.
use crate::Result;
use aidash_domain::semantic::indexing::{ImageChange, ImageSlots};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ImagePublication {
	pub workspace: Uuid,
	pub area: Uuid,
	pub generation: i64,
	pub sequence: i64,
	pub changes: Vec<ImageChange>,
	pub authority: Value,
	pub max_images: usize,
	pub slots: ImageSlots,
	/// True only when a requested rebuild has consumed the final queued event.
	pub rebuild_after: bool,
}

#[async_trait]
pub trait ImagePublicationSession: Send {
	/// Load the earliest publication after locking its Workspace and index.
	async fn load(&mut self) -> Result<Option<ImagePublication>>;
	/// Persist entry transitions and ledger state, then acknowledge the event.
	async fn store(
		&mut self,
		publication: &ImagePublication,
		before: &ImageSlots,
		after: &ImageSlots,
	) -> Result<()>;
	async fn commit(self: Box<Self>) -> Result<()>;
}
