//! The Research account holder's account terms. Every attempt of a priced
//! operation is attributed to this account and priced before dispatch.
use super::Operation;
use crate::{Error, Result};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The largest amount, in micro-USD, account terms may name. Charges are
/// recorded as signed 64-bit amounts; every admitted request keeps a month's
/// charges within the monthly limit, so their sum stays representable too.
pub const MAX_MICRO_USD: u64 = i64::MAX as u64;

/// Research account terms: what whoever runs the Deployment records for the
/// account a Research provider's requests are sent through and charged to.
/// This record is an admission prerequisite, not proof that an agreement has
/// been executed; whoever records it checks the actual agreement. The key the
/// account is reached with is named by the provider's declaration, never here,
/// because these terms are shown to Tenants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AccountTerms {
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
	pub price_effective_at: DateTime<Utc>,
	pub price_review_after: DateTime<Utc>,
	/// The price of one Web search, present exactly when the provider
	/// searches. Zero is a free request that is still attributed.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub search_price_micro_usd: Option<u64>,
	/// The price of one Page reading, present exactly when the provider reads
	/// pages.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub fetch_price_micro_usd: Option<u64>,
	pub monthly_base_fee_micro_usd: u64,
	pub monthly_limit_micro_usd: u64,
}

impl AccountTerms {
	/// Whether these are complete terms for a provider that implements
	/// `operations`, whatever the date: startup refuses terms that fail here.
	pub fn validate(&self, operations: &[Operation]) -> Result<()> {
		let priced = |operation| operations.contains(&operation);
		let highest = self
			.search_price_micro_usd
			.max(self.fetch_price_micro_usd)
			.unwrap_or(0);
		if self.account_id.trim().is_empty()
			|| self.agreement_reference.trim().is_empty()
			|| self.verified_by.trim().is_empty()
			|| self.review_after <= self.verified_at
			|| self.price_review_after <= self.price_effective_at
			|| !self.storage_permitted
			|| !self.query_training_prohibited
			|| !self.evaluation_permitted
			|| self.retention_days == 0
			|| self.deletion_after_termination_days == 0
			// Each charged operation the provider implements has a price, and
			// no other operation has one.
			|| self.search_price_micro_usd.is_some() != priced(Operation::Search)
			|| self.fetch_price_micro_usd.is_some() != priced(Operation::Fetch)
			// The limit also bounds the base fee and every price.
			|| self.monthly_limit_micro_usd > MAX_MICRO_USD
			|| self.monthly_limit_micro_usd
				< self.monthly_base_fee_micro_usd.saturating_add(highest)
		{
			return Err(Error::Invalid(
				"research account terms are incomplete".into(),
			));
		}
		Ok(())
	}

	/// Whether the verification and the prices are current at `now`. Terms
	/// that lapse after startup leave their provider not set up.
	pub fn current_at(&self, now: DateTime<Utc>) -> Result<()> {
		if self.verified_at > now
			|| self.review_after <= now
			|| self.price_effective_at > now
			|| self.price_review_after <= now
		{
			return Err(Error::Invalid(
				"research account terms are unavailable".into(),
			));
		}
		Ok(())
	}

	/// The recorded price of one `operation` request, if it is priced.
	pub fn price(&self, operation: Operation) -> Option<u64> {
		match operation {
			Operation::Search => self.search_price_micro_usd,
			Operation::Fetch => self.fetch_price_micro_usd,
			Operation::Find => None,
		}
	}

	pub fn attribution(&self, operation: Operation) -> Option<CostAttribution> {
		Some(CostAttribution {
			account_id: self.account_id.clone(),
			price_micro_usd: self.price(operation)?,
		})
	}

	/// Whether one more `operation` request fits the monthly limit, given the
	/// request charges already attributed to this account in the current
	/// month. An operation without a price is never admitted.
	pub fn admits(&self, operation: Operation, spent_this_month_micro_usd: u64) -> bool {
		self.price(operation).is_some_and(|price| {
			self.monthly_base_fee_micro_usd
				.saturating_add(spent_this_month_micro_usd)
				.saturating_add(price)
				<= self.monthly_limit_micro_usd
		})
	}

	/// The request charges the account may hold in one month.
	pub fn request_allowance_micro_usd(&self) -> u64 {
		self.monthly_limit_micro_usd
			.saturating_sub(self.monthly_base_fee_micro_usd)
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
