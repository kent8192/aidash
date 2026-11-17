//! Limits a Tier puts on a Tenant's Nodes. Community sets none; the Enterprise
//! build supplies the Tier (#242).
use crate::Result;
use aidash_domain::tenancy::TenantName;
use async_trait::async_trait;

#[async_trait]
pub trait TierLimits: Send + Sync {
	/// The most Nodes `tenant`'s Tier allows, or `None` when the Tier sets no limit.
	async fn nodes(&self, tenant: &TenantName) -> Result<Option<u32>>;
	/// The most Schedules `tenant`'s Tier allows, or `None` when the Tier sets
	/// no limit. It only lowers the Tenant's own limit (#557 decision 16).
	async fn schedules(&self, _tenant: &TenantName) -> Result<Option<u32>> {
		Ok(None)
	}
}

/// No Tier limits: the Tenancy Rule alone applies.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoTierLimits;

#[async_trait]
impl TierLimits for NoTierLimits {
	async fn nodes(&self, _: &TenantName) -> Result<Option<u32>> {
		Ok(None)
	}
}
