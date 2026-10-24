//! Evaluation uses the caller's current locked snapshot and complete delegated subject chain.
use crate::{Result, authorization::Snapshot};
use aidash_domain::policy::{Decision, Evaluation};
use async_trait::async_trait;
use serde_json::Value;
#[async_trait]
pub trait AuthorizationLease: Send {
	fn snapshot(&self) -> &Snapshot;
	fn subjects(&self) -> &[String];
	fn environment(&self) -> &Value;
	/// The adapter retains the caller's ordinary or durable worker audit semantics.
	async fn record(&mut self, records: &[(Evaluation, Decision)]) -> Result<()>;
}
