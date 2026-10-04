//! Receiver scopes keep identity mapping, reservation and publication in one owned transaction.
use super::{TransferRepository, TransferScope};
use crate::Result;
use aidash_domain::capabilities::{
	operations::MountedFile, records::Record, sessions::Area, transfer::Description,
};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait ReceiverScope: TransferScope {
	async fn recipient(&mut self, description: &Description, node_id: &str)
	-> Result<Option<Area>>;
	async fn admitted(&mut self, area: &Area, version: &str) -> Result<Option<Uuid>>;
	async fn recipient_rows(&mut self, cursor: Option<Uuid>) -> Result<Vec<Area>>;
	async fn versions(&mut self, area: &Area) -> Result<Vec<String>>;
	async fn reserve(&mut self, bytes: i64) -> Result<()>;
	async fn put_staging(&mut self, bytes: &[u8]) -> Result<(Uuid, String)>;
	async fn begin_received(&mut self, area: Uuid, size: u64) -> Result<()>;
	async fn write_pending(&mut self, bytes: &[u8]) -> Result<()>;
	async fn finish_pending(&mut self, digest: &str) -> Result<(Uuid, String)>;
	async fn read_file(&mut self, file: &MountedFile) -> Result<Vec<u8>>;
	async fn publish(&mut self, area: &mut Area) -> Result<()>;
	async fn event(&mut self, workspace: Uuid, kind: &str, data: serde_json::Value) -> Result<()>;
}
#[async_trait]
pub trait ReceiverRepository: TransferRepository {
	fn node_id(&self) -> &str;
	async fn incoming(&self, id: Uuid) -> Result<Record>;
	async fn status_snapshot(&self, id: Uuid) -> Result<Record>;
	async fn begin_peer(
		&self,
		source: &str,
		tenant: &str,
		subject: &str,
	) -> Result<Box<dyn ReceiverScope + '_>>;
}
