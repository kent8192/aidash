//! Workspace deletion extends the same cleanup transaction with ordered owned-area reads.
use super::cleanup::CleanupScope;
use crate::Result;
use aidash_domain::capabilities::sessions::Area;
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait ThreadScope: CleanupScope {
	/// Lock the Workspace row, so a Run admitted to it cannot commit behind
	/// its deletion.
	async fn lock_for_deletion(&mut self, workspace: Uuid) -> Result<()>;
	async fn owned_areas(&mut self, workspace: Uuid, offset: u64) -> Result<Vec<Area>>;
	/// Remove the deleted Workspace's attachments in the tombstone transaction.
	async fn delete_attachments(&mut self, workspace: Uuid) -> Result<()>;
	async fn cancel_generation(&mut self, area: &Area) -> Result<()>;
	/// Stop the Duties whose own Workspace this is (#557, #1011).
	async fn stop_schedules(&mut self, _workspace: Uuid) -> Result<()> {
		Ok(())
	}
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()>;
}
