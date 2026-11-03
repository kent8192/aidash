//! Native scans and receipt updates do not grant a worker execution authority.
use super::reconciliation::OperationReconciliationRepository;
use crate::Result;
use aidash_domain::capabilities::operations::processing::Receipt;
use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::watch;
use uuid::Uuid;
#[async_trait]
pub trait OutputTransfer: Send {
	async fn request(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value>;
	async fn begin_output(&mut self, size: u64) -> Result<()>;
	async fn write_output(&mut self, bytes: &[u8]) -> Result<()>;
	async fn finish_output(&mut self, expected: &str) -> Result<(Uuid, String)>;
}
#[async_trait]
impl<T: super::reconciliation::OperationReconciliationScope + ?Sized> OutputTransfer for T {
	async fn request(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
		super::reconciliation::OperationReconciliationScope::request(self, method, path, body).await
	}
	async fn begin_output(&mut self, size: u64) -> Result<()> {
		super::reconciliation::OperationReconciliationScope::begin_output(self, size).await
	}
	async fn write_output(&mut self, bytes: &[u8]) -> Result<()> {
		super::reconciliation::OperationReconciliationScope::write_output(self, bytes).await
	}
	async fn finish_output(&mut self, expected: &str) -> Result<(Uuid, String)> {
		super::reconciliation::OperationReconciliationScope::finish_output(self, expected).await
	}
}
/// Downloads are provisional and hold no policy/gate transaction. Commit
/// revalidates the exact stopped operation and adopts its output atomically.
#[async_trait]
pub trait ReceiptOutputScope: OutputTransfer {
	fn limits(&self) -> aidash_domain::capabilities::operations::reconciliation::Limits;
	async fn put(&mut self, kind: &str, bytes: &[u8]) -> Result<(Uuid, String)>;
	async fn commit(self: Box<Self>, proof: &str, output: Value) -> Result<()>;
}
#[async_trait]
pub trait OperationProcessingRepository:
	OperationReconciliationRepository + super::pod_usage::PodUsageRepository
{
	async fn active_operations(&self) -> Result<Vec<Uuid>>;
	async fn receipts(&self, after: Uuid) -> Result<Vec<Receipt>>;
	async fn mark_acknowledged(&self, id: Uuid) -> Result<()>;
	async fn storage_blocked(&self, id: Uuid, detail: Value) -> Result<()>;
	async fn runner_request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value>;
	async fn begin_receipt_output(
		&self,
		receipt: &Receipt,
	) -> Result<Box<dyn ReceiptOutputScope + '_>>;
}
#[async_trait]
pub trait CapabilityBackgroundJobs: Send + Sync {
	async fn network(&self, stopping: watch::Receiver<bool>) -> Result<()>;
	async fn references(&self, stopping: watch::Receiver<bool>) -> Result<()>;
	async fn cleanup(&self, stopping: watch::Receiver<bool>) -> Result<()>;
	async fn reclamation(&self, stopping: watch::Receiver<bool>) -> Result<()>;
}
