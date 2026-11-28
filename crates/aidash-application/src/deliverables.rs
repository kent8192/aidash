//! Read the Drive versions that a Task published before its Completion Message.

use crate::Result;
use async_trait::async_trait;
use uuid::Uuid;

pub use aidash_domain::deliverable::DriveVersionRef;

#[async_trait]
pub trait DeliverablePort: Send + Sync {
	async fn published_versions(&self, task: Uuid) -> Result<Vec<DriveVersionRef>>;
}
