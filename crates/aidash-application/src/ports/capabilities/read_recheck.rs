//! What the 30 second Read Recheck asks of its adapters (ADR 0116).
use crate::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Something that keeps running between a Run's worker boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
	Operation(Uuid),
	/// A Run with a potentially live stdio MCP session.
	StdioSession(Uuid),
	LivePreview(Uuid),
	/// A durable revocation whose Pod stop has not yet been confirmed.
	RevokedRun(Uuid),
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
	/// Hold back a Live preview's responses while its reads cannot be confirmed.
	/// Returns since when they have been held back, or `None` once the preview
	/// no longer serves (ADR 0129).
	async fn withhold(&self, preview: Uuid) -> Result<Option<DateTime<Utc>>>;
	/// Let a withheld Live preview serve again once a check is confirmed.
	async fn serve_again(&self, preview: Uuid) -> Result<()>;
	/// Stop a Live preview whose reads could not be confirmed for too long.
	async fn stop_unconfirmed(&self, preview: Uuid) -> Result<()>;
}
