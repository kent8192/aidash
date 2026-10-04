//! Worker scopes retain live authority, visibility and the already committed lease.
use crate::{Error, Result, ports::ExecutionRecoveryStore, recovery::ExecutionFailure};
use aidash_domain::{RunControl, RunMetadata};
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait WorkerLeases: Send + Sync {
	async fn current_id(&self, token: Uuid) -> Result<Uuid>;
	async fn renew(&self, run: Uuid, token: Uuid, seconds: i32) -> Result<bool>;
	fn transient(&self, error: &Error) -> bool;
	/// Read committed control independently of the current authority transaction.
	async fn control(&self, run: Uuid) -> Result<RunControl>;
}

#[async_trait]
pub trait WorkerStep: Send {
	fn metadata(&self) -> RunMetadata;
	fn recovery_store(&self) -> &dyn ExecutionRecoveryStore;
	fn classify_failure(&self, error: Error) -> ExecutionFailure;
	async fn cancel_scoped(&mut self, token: Uuid) -> Result<bool>;
	async fn admit(&mut self) -> Result<()>;
	async fn invoke(&mut self, token: Uuid) -> Result<()>;
	/// Finish the retained authority boundary with the original operation outcome.
	async fn finish_authority(&mut self, result: Result<()>) -> Result<()>;
	async fn resume_visibility(&mut self) -> Result<()>;
}
