//! How many Nodes a Tenant has, for the Tier's Node limit.
use aidash_server::Result;
use async_trait::async_trait;
use std::collections::BTreeMap;

#[async_trait]
pub trait NodeCount: Send + Sync {
	async fn nodes(&self, tenant: &str) -> Result<u32>;
}

/// Current live Nodes, including the default, for attachment and Tier changes.
pub struct StoredNodes(pub aidash_server::database::native::Pool);

#[async_trait]
impl NodeCount for StoredNodes {
	async fn nodes(&self, tenant: &str) -> Result<u32> {
		self.0
			.scoped(
				&aidash_application::tenancy::TenancyScope::tenant(tenant)?,
				async |tx| {
					aidash_server::authorization::services::nodes::count_live(&mut **tx, tenant)
						.await
				},
			)
			.await
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
