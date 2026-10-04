//! One withdrawal scope owns the Area-before-operation locks until commit or drop.
use crate::Result;
use aidash_domain::capabilities::operations::withdrawal::{Change, Snapshot};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait OperationWithdrawalScope: Send {
	fn snapshot(&self) -> Snapshot;
	/// Apply both updates and commit atomically. Errors and cancellation roll back.
	async fn commit(self: Box<Self>, change: Change) -> Result<()>;
}
#[async_trait]
pub trait OperationWithdrawalRepository: Send + Sync {
	async fn lock(&self, area: Uuid, operation: Uuid) -> Result<Box<dyn OperationWithdrawalScope>>;
	async fn cancel(&self, operation: Uuid) -> Result<Value>;
}
