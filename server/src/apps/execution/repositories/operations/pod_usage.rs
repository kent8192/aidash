//! Trusted runner housekeeping outlives Run authority and all Tenant-owned rows.
use super::reconciliation::Repository;
use aidash_application::{
	Error, Result,
	ports::capabilities::{
		pod_usage::{AllocationPage, AllocationReceipt, PodUsageRepository},
		processing::OperationProcessingRepository,
	},
	usage::PodTime,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
	instance: Uuid,
	receipts: Vec<AllocationReceipt>,
	next_after: String,
	pending_evidence: u64,
}
#[async_trait]
impl PodUsageRepository for Repository<'_> {
	async fn pending_allocations(&self, after: Option<Uuid>) -> Result<AllocationPage> {
		let health = self.runner_request("GET", "/v1/health", None).await?;
		let page = self
			.runner_request(
				"GET",
				&format!(
					"/v1/allocations/receipts?limit=16&after={}",
					after.map(|id| id.to_string()).unwrap_or_default()
				),
				None,
			)
			.await?;
		let page: Page = serde_json::from_value(page)?;
		if health["instance"].as_str() != Some(page.instance.to_string().as_str()) {
			return Err(Error::External(
				"allocation journal identity unavailable".into(),
			));
		}
		crate::apps::execution::models::runner_allocation_journal::confirm(
			&self.store.pool,
			&self.store.deployment,
			page.instance,
		)
		.await
		.map_err(Error::from)?;
		let next_after = if page.next_after.is_empty() {
			None
		} else {
			Some(
				Uuid::parse_str(&page.next_after)
					.map_err(|_| Error::External("invalid allocation cursor".into()))?,
			)
		};
		Ok(AllocationPage {
			instance: page.instance,
			receipts: page.receipts,
			next_after,
			pending_evidence: page.pending_evidence,
		})
	}
	async fn deliver_allocation(&self, event: PodTime) -> Result<()> {
		self.store.usage.pod_time(event).await
	}
	async fn acknowledge_allocation(&self, receipt: &AllocationReceipt) -> Result<()> {
		let ack = self
			.runner_request(
				"POST",
				&format!("/v1/allocations/{}/ack", receipt.allocation_id),
				Some(json!({"instance":receipt.runner_instance, "digest":receipt.digest})),
			)
			.await?;
		if ack
			!= json!({"allocation_id":receipt.allocation_id,"instance":receipt.runner_instance,"digest":receipt.digest,"acknowledged":true})
		{
			return Err(Error::External(
				"allocation acknowledgement identity mismatch".into(),
			));
		}
		Ok(())
	}
}
