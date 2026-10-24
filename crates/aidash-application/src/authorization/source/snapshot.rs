//! A prepared grant never permits disclosure after current source authority has expired.
use crate::{Error, Result, ports::authorization::source::snapshot::SnapshotScope};
use aidash_domain::WorkspaceSnapshot;
use uuid::Uuid;
/// The caller retains the current description lease through completion and audit.
pub async fn read<S: SnapshotScope + ?Sized>(
	scope: &mut S,
	grant: Uuid,
	workspace: Uuid,
) -> Result<WorkspaceSnapshot> {
	scope.select_read_grant(grant);
	let snapshot = scope.workspace_snapshot(workspace).await?;
	if !scope.snapshot_grant_live(grant).await? {
		return Err(Error::Forbidden);
	}
	Ok(snapshot)
}
#[cfg(test)]
mod tests;
