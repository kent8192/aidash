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

/// The native adapter retains its current area, authority and file snapshot through all reads.
#[async_trait]
pub trait SelectedMediaScope: Send {
	async fn current_files(&mut self) -> Result<Vec<aidash_domain::media::SelectedFile>>;
	async fn subnode_file(
		&mut self,
		_workspace: Uuid,
		_area: Uuid,
		_file: Uuid,
	) -> Result<aidash_domain::media::SelectedFile> {
		Err(crate::Error::Forbidden)
	}
	async fn read(&mut self, file: Uuid) -> Result<Vec<u8>>;
	/// The media type and bytes of the Run's page-image Evidence fragment
	/// `reference`; absent once its evidence is revoked (ADR 0053).
	async fn page_image(&mut self, reference: &str) -> Result<Option<(String, Vec<u8>)>>;
}
