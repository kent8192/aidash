//! Read the Drive versions that a Task published before its Completion Message.

use crate::Result;
use async_trait::async_trait;
use uuid::Uuid;

// CONTRACT(#1019): Worker B owns this module and the canonical DriveVersionRef.
pub use aidash_domain::drive::VersionRef as DriveVersionRef;

#[async_trait]
pub trait DeliverablePort: Send + Sync {
	async fn published_versions(&self, task: Uuid) -> Result<Vec<DriveVersionRef>>;
}
