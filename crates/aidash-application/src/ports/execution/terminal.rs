//! Failure and terminal-input outboxes remain independent of runnable activation.
use crate::Result;
use aidash_domain::{RunMetadata, Task, TaskStatus};
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait FailureScope: Send {
	fn metadata(&self) -> &RunMetadata;
	fn target(&self) -> TaskStatus;
	async fn remote_grant(&mut self) -> Result<bool>;
	async fn admit(&mut self) -> Result<()>;
	async fn deliver_inputs(&mut self) -> Result<()>;
	async fn task(&mut self) -> Result<Task>;
	async fn transition(&mut self, status: TaskStatus) -> Result<Task>;
	async fn finish_authority(&mut self, result: Result<TaskStatus>) -> Result<TaskStatus>;
	async fn pause_authority(&mut self, identity_unavailable: bool) -> Result<()>;
	async fn finish(&mut self, result: Result<TaskStatus>) -> Result<()>;
}

#[async_trait]
pub trait TerminalRepository: Send + Sync {
	/// Claim failure responsibility under a retained ordinary-state visibility lease.
	async fn claim_failure(
		&self,
		token: Uuid,
		seconds: i32,
	) -> Result<Option<Box<dyn FailureScope>>>;
	async fn pending_inputs(&self) -> Result<Option<RunMetadata>>;
	async fn deliver_inputs(&self, run: &RunMetadata) -> Result<()>;
	async fn defer_inputs(&self, run: Uuid) -> Result<()>;
}
