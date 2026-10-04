//! Existing scoped grants and atomic Run writes remain behind the worker cancellation port.
use crate::Result;
use aidash_domain::{Run, RunMetadata};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait ScopedCancellationRepository: Send + Sync {
	async fn remote_grant(&self, run: &RunMetadata) -> Result<bool>;
	async fn local_grant(&self, run: &RunMetadata) -> Result<bool>;
	/// Preserve the existing worker token, revision/input fences and event transaction.
	async fn save_receiver(&self, run: &Run, token: Uuid, event: &str) -> Result<()>;
	async fn cancel_local(&self, run: &Run, token: Uuid) -> Result<()>;
}
