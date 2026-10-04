//! Native scans and receipt updates do not grant a worker execution authority.
use super::reconciliation::OperationReconciliationRepository;
use crate::Result;
use aidash_domain::capabilities::operations::processing::Receipt;
use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::watch;
use uuid::Uuid;
#[async_trait]
pub trait OperationProcessingRepository: OperationReconciliationRepository {
	async fn active_operations(&self) -> Result<Vec<Uuid>>;
	async fn receipts(&self, after: Uuid) -> Result<Vec<Receipt>>;
	async fn mark_acknowledged(&self, id: Uuid) -> Result<()>;
	async fn storage_blocked(&self, id: Uuid, detail: Value) -> Result<()>;
	async fn runner_request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value>;
}
#[async_trait]
pub trait CapabilityBackgroundJobs: Send + Sync {
	async fn network(&self, stopping: watch::Receiver<bool>) -> Result<()>;
	async fn references(&self, stopping: watch::Receiver<bool>) -> Result<()>;
	async fn cleanup(&self, stopping: watch::Receiver<bool>) -> Result<()>;
	async fn reclamation(&self, stopping: watch::Receiver<bool>) -> Result<()>;
}
