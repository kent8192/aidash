//! What the 30 second Read Recheck asks of its adapters (ADR 0116).
use crate::Result;
use async_trait::async_trait;
use uuid::Uuid;

/// Something that keeps running between a Run's worker boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
	Operation(Uuid),
	LivePreview(Uuid),
}

/// What one Read Recheck found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
	Allowed,
	Refused,
	/// A Node holding something the Run read did not answer. Nothing was refused.
	Unconfirmed,
}

#[async_trait]
pub trait ReadRecheckRepository: Send + Sync {
	async fn targets(&self) -> Result<Vec<Target>>;
	async fn check(&self, target: Target) -> Result<Outcome>;
	/// Withdraw the command or stop the Live preview as an authority revocation.
	async fn stop_refused(&self, target: Target) -> Result<()>;
	/// Stop a Live preview whose reads could not be confirmed for too long.
	async fn stop_unconfirmed(&self, preview: Uuid) -> Result<()>;
}
