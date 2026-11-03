//! Authority-independent provider allocation receipt delivery. No Tenant row is required.
use crate::{Result, usage::PodTime};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationReceipt {
	pub protocol: String,
	pub allocation_id: Uuid,
	pub runner_instance: Uuid,
	pub tenant: String,
	pub area_id: Uuid,
	pub epoch: u64,
	pub operation_id: Uuid,
	pub started_at: String,
	pub ended_at: String,
	pub evidence: AllocationEvidence,
	pub digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationEvidence {
	pub pod_uid: Uuid,
	pub start_time: String,
	pub containers: Vec<ContainerStop>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerStop {
	pub name: String,
	pub finished_at: String,
}
#[derive(Clone, Debug)]
pub struct AllocationPage {
	pub instance: Uuid,
	pub receipts: Vec<AllocationReceipt>,
	pub next_after: Option<Uuid>,
	pub pending_evidence: u64,
}
#[async_trait]
pub trait PodUsageRepository: Send + Sync {
	async fn pending_allocations(&self, after: Option<Uuid>) -> Result<AllocationPage>;
	async fn deliver_allocation(&self, event: PodTime) -> Result<()>;
	async fn acknowledge_allocation(&self, receipt: &AllocationReceipt) -> Result<()>;
}
