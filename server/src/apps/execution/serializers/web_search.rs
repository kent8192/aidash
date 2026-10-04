use serde::{Deserialize, Serialize};
// Serializable contracts for harness.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;

/// Operator-attested terms for one paid Brave account. This record is an
/// admission prerequisite, not proof that an agreement has been executed.
/// The actual agreement must be checked and recorded by the operator.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
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
	pub credential_env: String,
	pub price_effective_at: DateTime<Utc>,
	pub price_review_after: DateTime<Utc>,
	pub request_price_micro_usd: u64,
	pub monthly_base_fee_micro_usd: u64,
	pub monthly_limit_micro_usd: u64,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Language {
	Ja,
	En,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DateRange {
	pub from: String,
	pub to: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum Freshness {
	Named(String),
	Range(DateRange),
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchInput {
	pub query: String,
	#[serde(default)]
	pub language: Option<Language>,
	#[serde(default)]
	pub country: Option<String>,
	#[serde(default)]
	pub count: Option<u8>,
	#[serde(default)]
	pub page: Option<u8>,
	#[serde(default)]
	pub freshness: Option<Freshness>,
	#[serde(default)]
	pub include_domains: Vec<String>,
	#[serde(default)]
	pub exclude_domains: Vec<String>,
}
