//! Whether new work may begin in a Tenant on Cloud. The gate decides from
//! state Aidash already holds: the Billing Account's Subscription as it was
//! last reported, the dunning grace period, and metering records. It never
//! calls the payment processor, so a processor outage leaves running Tenants
//! starting Runs and Sandboxed tools. Reading and export are not gated here,
//! and nothing deletes a Tenant.
use super::meter::month_start;
use super::payments::PaymentState;
use super::rules::{self, Cover};
use super::settings::Settings;
use super::store;
use aidash_application::usage::{Admission, Verdict, Work};
use aidash_domain::offering::Offering;
use aidash_server::database::native::Pool;
use async_trait::async_trait;
use chrono::Utc;
use std::sync::Arc;

pub const FREE_USAGE_EXHAUSTED: &str = "free_usage_exhausted";

pub struct Gate {
	pub pool: Pool,
	pub settings: Arc<Settings>,
	pub payments: Arc<dyn PaymentState>,
	pub offering: Offering,
}

#[async_trait]
impl Admission for Gate {
	async fn admit(&self, tenant: &str, work: Work) -> aidash_application::Result<Verdict> {
		// Dedicated meters and reports usage but charges through its contract.
		if self.offering != Offering::Cloud {
			return Ok(Verdict::Admit);
		}
		let now = Utc::now();
		let Some(facts) = store::tenant_facts(&self.pool, tenant).await? else {
			// No such Tenant: every other boundary refuses it.
			return Ok(Verdict::Admit);
		};
		let subscription = match facts.account {
			Some(account) => self.payments.subscription(account).await?,
			None => None,
		};
		let free_eligible = facts.personal
			&& facts
				.mappings
				.iter()
				.all(|mapping| Some(*mapping) == facts.creator);
		if work == Work::Member {
			// Joining a Personal Tenant that someone already holds ends the Free
			// Tier: it needs an attached account with an active paid Subscription.
			if !facts.personal || facts.mappings.is_empty() {
				return Ok(Verdict::Admit);
			}
			return Ok(
				match rules::cover(&self.settings, subscription.as_ref(), false, now) {
					Ok(_) => Verdict::Admit,
					Err(denied) => Verdict::refuse(denied.code()),
				},
			);
		}
		match rules::cover(&self.settings, subscription.as_ref(), free_eligible, now) {
			Err(denied) => Ok(Verdict::refuse(denied.code())),
			Ok(Cover::Paid(_)) => Ok(Verdict::Admit),
			Ok(Cover::Free(tier)) => {
				let millis = store::pod_millis_since(&self.pool, tenant, month_start(now)).await?;
				let storage = store::latest_storage_bytes(&self.pool, tenant).await?;
				Ok(if rules::free_exhausted(tier, millis / 1000, storage) {
					Verdict::refuse(FREE_USAGE_EXHAUSTED)
				} else {
					Verdict::Admit
				})
			}
		}
	}
}
