//! Thread deletion extends the same cleanup transaction with ordered owned-area reads.
use super::cleanup::CleanupScope;
use crate::Result;
use aidash_domain::{capabilities::sessions::Area, workspaces::channels::ChannelThread};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait ThreadScope: CleanupScope {
	async fn channel(&mut self, workspace: Uuid, thread: Uuid) -> Result<Option<ChannelThread>>;
	async fn owned_areas(
		&mut self,
		workspace: Uuid,
		thread: Uuid,
		offset: u64,
	) -> Result<Vec<Area>>;
	/// Remove the deleted thread's attachments in the tombstone transaction.
	async fn delete_attachments(&mut self, workspace: Uuid, thread: Uuid, root: Uuid)
	-> Result<()>;
	async fn cancel_generation(&mut self, area: &Area) -> Result<()>;
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()>;
}
