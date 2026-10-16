//! The Research account holder's account terms. Every search attempt is
//! attributed to this account and priced before dispatch.
use crate::{Error, Result};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The largest amount, in micro-USD, an account profile may name. Charges are
/// recorded as signed 64-bit amounts; every admitted request keeps a month's
/// charges within the monthly limit, so their sum stays representable too.
pub const MAX_MICRO_USD: u64 = i64::MAX as u64;

/// Deployment Administrator-attested terms for one paid search account. This record is an
/// admission prerequisite, not proof that an agreement has been executed.
/// A Deployment Administrator checks and records the actual agreement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AccountProfile {
	pub account_id: String,
	pub agreement_reference: String,
	pub verified_by: String,
	pub verified_at: DateTime<Utc>,
	pub review_after: DateTime<Utc>,
	pub storage_permitted: bool,
	pub query_training_prohibited: bool,
	pub evaluation_permitted: bool,
	pub retention_days: u16,
	pub deletion_after_termination_days: u16,
	/// The `AIDASH_SECRET_*` tool secret holding the provider key.
	pub credential_env: String,
	pub price_effective_at: DateTime<Utc>,
	pub price_review_after: DateTime<Utc>,
	pub request_price_micro_usd: u64,
	pub monthly_base_fee_micro_usd: u64,
	pub monthly_limit_micro_usd: u64,
}

impl AccountProfile {
	pub fn validate_at(&self, now: DateTime<Utc>) -> Result<()> {
		if self.account_id.trim().is_empty()
			|| self.agreement_reference.trim().is_empty()
			|| self.verified_by.trim().is_empty()
			|| self.verified_at > now
			|| self.review_after <= now
			|| self.review_after <= self.verified_at
			|| self.price_effective_at > now
			|| self.price_review_after <= now
			|| self.price_review_after <= self.price_effective_at
			|| !self.storage_permitted
			|| !self.query_training_prohibited
			|| !self.evaluation_permitted
			|| self.retention_days == 0
			|| self.deletion_after_termination_days == 0
			|| self.request_price_micro_usd == 0
			// The limit also bounds the base fee and the request price.
			|| self.monthly_limit_micro_usd > MAX_MICRO_USD
			|| self.monthly_limit_micro_usd
				< self
					.monthly_base_fee_micro_usd
					.saturating_add(self.request_price_micro_usd)
		{
			return Err(Error::Invalid(
				"search account terms are unavailable".into(),
			));
		}
		crate::configuration::validate_secret_reference(&self.credential_env)
	}

	pub fn attribution(&self) -> CostAttribution {
		CostAttribution {
			account_id: self.account_id.clone(),
			price_micro_usd: self.request_price_micro_usd,
		}
	}

	/// Whether one more request fits the monthly limit, given the request
	/// charges already attributed to this account in the current month.
	pub fn admits(&self, spent_this_month_micro_usd: u64) -> bool {
		self.monthly_base_fee_micro_usd
			.saturating_add(spent_this_month_micro_usd)
			.saturating_add(self.request_price_micro_usd)
			<= self.monthly_limit_micro_usd
	}
}

/// The charge one attempt attributes to the Research account holder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostAttribution {
	pub account_id: String,
	pub price_micro_usd: u64,
}

/// Start of the UTC calendar month containing `now`; monthly limits reset here.
pub fn month_start(now: DateTime<Utc>) -> DateTime<Utc> {
	Utc.with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
		.single()
		.expect("the first of a month is unambiguous in UTC")
}

#[cfg(test)]
mod tests;
