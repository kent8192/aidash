//! Run attachments are read under the existing caller-owned transaction.
use crate::Result;
use async_trait::async_trait;
use uuid::Uuid;
#[derive(Clone)]
pub struct MediaAttachment {
	pub filename: String,
	pub media_type: String,
	pub sha256: String,
	pub size_bytes: i64,
	pub content: Vec<u8>,
}
#[async_trait]
pub trait MediaAttachments: Send {
	async fn attachments(&mut self, workspace: Uuid, message: Uuid)
	-> Result<Vec<MediaAttachment>>;
}
#[async_trait]
pub trait AuthorizedMediaScope: MediaAttachments {
	async fn authorize_message(&mut self, workspace: Uuid, message: Uuid) -> Result<()>;
}
#[async_trait]
pub trait OperatorMediaRepository: Send + Sync {
	fn node_id(&self) -> &str;
	async fn begin(&self) -> Result<Box<dyn MediaTransaction>>;
}
#[async_trait]
pub trait MediaTransaction: MediaAttachments {
	async fn commit(self: Box<Self>) -> Result<()>;
}

/// The native adapter retains its current area, authority and file snapshot through all reads.
#[async_trait]
pub trait SelectedMediaScope: Send {
	async fn current_files(&mut self) -> Result<Vec<aidash_domain::media::SelectedFile>>;
	async fn read(&mut self, file: Uuid) -> Result<Vec<u8>>;
}
