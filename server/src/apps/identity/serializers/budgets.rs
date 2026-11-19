//! Budgets (#1012, ADR 0143).
use aidash_domain::budgets::{Definition, Standing};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One Budget with its spend in the current window. Spend is the cost
/// providers reported for model calls, in billionths of a credit (one credit
/// is one US dollar on OpenRouter); `unpriced_calls` counts calls whose cost
/// no provider reported, which no limit can count.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BudgetView {
	pub id: Uuid,
	#[serde(flatten)]
	pub definition: Definition,
	pub revision: i64,
	pub created_at: DateTime<Utc>,
	pub created_by: String,
	pub updated_at: DateTime<Utc>,
	pub window_start: DateTime<Utc>,
	pub next_reset: Option<DateTime<Utc>>,
	pub spent_nanocredits: i64,
	pub unpriced_calls: i64,
	pub standing: Standing,
}

/// Changes a Budget when `expected_revision` is still current (409
/// otherwise). Its scope never changes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BudgetUpdate {
	#[serde(flatten)]
	pub definition: Definition,
	pub expected_revision: i64,
}
