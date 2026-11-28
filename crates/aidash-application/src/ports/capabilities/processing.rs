//! Native scans and receipt updates do not grant a worker execution authority.
use super::reconciliation::OperationReconciliationRepository;
use crate::Result;
use aidash_domain::capabilities::operations::processing::Receipt;
use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::watch;
use uuid::Uuid;
/// Downloads are provisional and hold no policy/gate transaction. Commit
/// revalidates the exact stopped operation and adopts its output atomically.
#[async_trait]
pub trait ReceiptOutputScope: Send {
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
	/// Commands and Python cells that run in Execution Pods (#745). None where
	/// the Deployment has no Execution Pods.
	fn pods(&self) -> Option<&dyn super::pod_operations::PodOperationRepository> {
		None
	}
}
#[async_trait]
pub trait CapabilityBackgroundJobs: Send + Sync {
	async fn network(&self, stopping: watch::Receiver<bool>) -> Result<()>;
	async fn references(&self, stopping: watch::Receiver<bool>) -> Result<()>;
	async fn reclamation(&self, stopping: watch::Receiver<bool>) -> Result<()>;
}
