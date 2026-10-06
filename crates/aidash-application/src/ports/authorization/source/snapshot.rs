//! A source snapshot reads under the already reconstructed grant's current authority.
use crate::Result;
use aidash_domain::WorkspaceSnapshot;
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait SnapshotScope: Send {
	fn select_read_grant(&mut self, grant: Uuid);
	async fn workspace_snapshot(&mut self, workspace: Uuid) -> Result<WorkspaceSnapshot>;
	/// Use database time on the same transaction after the complete snapshot read.
	async fn snapshot_grant_live(&mut self, grant: Uuid) -> Result<bool>;
}
