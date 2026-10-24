//! Snapshot ports borrow the original native source description transaction.
use crate::authorization::access::Access;
use aidash_application::{Result, ports::authorization::source::snapshot::SnapshotScope};
use aidash_domain::WorkspaceSnapshot;
use async_trait::async_trait;
use uuid::Uuid;
pub(crate) struct Scope<'a> {
	pub(crate) access: &'a mut Access,
}
#[async_trait]
impl SnapshotScope for Scope<'_> {
	fn select_read_grant(&mut self, grant: Uuid) {
		self.access.read_grant = Some(grant);
	}
	async fn workspace_snapshot(&mut self, workspace: Uuid) -> Result<WorkspaceSnapshot> {
		self.access
			.workspace_snapshot(workspace)
			.await
			.map_err(Into::into)
	}
	async fn snapshot_grant_live(&mut self, grant: Uuid) -> Result<bool> {
		super::persistence::live(self.access, grant)
			.await
			.map_err(Into::into)
	}
}
