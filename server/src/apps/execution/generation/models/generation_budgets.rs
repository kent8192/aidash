//! Persistent generation_budgets records.

use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "generation_budgets")]
#[derive(Serialize, Deserialize)]
pub struct GenerationBudget {
	#[field(primary_key = true)]
	pub request_id: uuid::Uuid,
	#[field]
	pub token_limit: i64,
	#[field(default = 0)]
	pub used_tokens: i64,
	#[field(default = 0)]
	pub compaction_call_limit: i64,
	#[field(default = 0)]
	pub compaction_calls: i64,
	#[field(default = 0)]
	pub embedding_call_limit: i64,
	#[field(default = 0)]
	pub embedding_calls: i64,
	#[field(default = 0)]
	pub summary_call_limit: i64,
	#[field(default = 0)]
	pub summary_calls: i64,
}

impl GenerationBudget {}
