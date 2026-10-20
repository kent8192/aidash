//! How many Nodes a Tenant has, for the Tier's Node limit.
use aidash_server::Result;
use async_trait::async_trait;
use std::collections::BTreeMap;

#[async_trait]
pub trait NodeCount: Send + Sync {
	async fn nodes(&self, tenant: &str) -> Result<u32>;
}

/// Every Tenant has its Default Node and no other until Nodes can be created;
/// the Node-aware count replaces this where they land.
pub struct DefaultNode;

#[async_trait]
impl NodeCount for DefaultNode {
	async fn nodes(&self, _: &str) -> Result<u32> {
		Ok(1)
	}
}

/// Fixed counts for tests, one Node for a Tenant not listed.
#[derive(Default)]
pub struct FixedNodes(pub BTreeMap<String, u32>);

#[async_trait]
impl NodeCount for FixedNodes {
	async fn nodes(&self, tenant: &str) -> Result<u32> {
		Ok(self.0.get(tenant).copied().unwrap_or(1))
	}
}
