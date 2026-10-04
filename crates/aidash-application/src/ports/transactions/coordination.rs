//! Durable coordinator storage and participant communication capabilities.
use crate::Result;
use aidash_domain::transactions::{
	CoordinatorTransition, Manifest,
	authority::Status,
	coordination::{LocalStatus, ParticipantOperation, Vote},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Own the recovery lease across peer I/O. Drop releases it on cancellation.
#[async_trait]
pub trait RecoveryScope: Send {
	/// Retain the persistence connection until this scope is released or dropped.
	async fn touch(&mut self) -> Result<()>;
	/// Update exactly this participant and clear the coordinator error atomically.
	async fn acknowledge(&mut self, node: &str, phase: &str) -> Result<()>;
	async fn record_error(&mut self, error: &str) -> Result<()>;
	/// Release the persistence connection before committing the recovery lease.
	/// Called even when an advance returns an error; a release error takes priority.
	async fn release(self: Box<Self>) -> Result<()>;
}

/// Own the candidate scan's persistence resources until the batch is finished.
pub trait RecoveryBatch: Send + Sync {
	fn ids(&self) -> &[Uuid];
}

#[async_trait]
pub trait CoordinatorRepository: Send + Sync {
	fn node_id(&self) -> &str;
	fn now(&self) -> DateTime<Utc>;
	async fn status(&self, id: Uuid) -> Result<Status>;
	async fn votes(&self, id: Uuid) -> Result<Vec<Vote>>;
	/// Arbitrate immutable decisions under the row lock and append audit history
	/// in the same transaction, retaining the existing before/after fault cuts.
	async fn transition(&self, id: Uuid, change: CoordinatorTransition, detail: &str)
	-> Result<()>;
	async fn acquire_recovery(&self, id: Uuid) -> Result<Box<dyn RecoveryScope>>;
	async fn recovery_batch(&self, aborted: bool) -> Result<Box<dyn RecoveryBatch>>;
	async fn issue_authority(&self, manifest: &Manifest, node: &str) -> Result<()>;
	async fn settle_authority(&self, id: Uuid, node: &str, phase: &str) -> Result<()>;
	async fn scoped(&self, id: Uuid) -> Result<bool>;
	async fn fault(&self, id: Uuid, point: &str) -> Result<()>;
}

#[async_trait]
pub trait ParticipantTransport: Send + Sync {
	async fn decision(&self, manifest: &Manifest) -> Result<Status>;
	async fn send(
		&self,
		manifest: &Manifest,
		node: &str,
		operation: ParticipantOperation,
	) -> Result<LocalStatus>;
}
