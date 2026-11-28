//! Persistent core_runs records.
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "core_runs")]
#[derive(Serialize, Deserialize)]
pub struct CoreRuns {
	#[field(primary_key = true)]
	pub run_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	/// Subject recorded when the capability scope was admitted.
	#[field(field_type = "text", null = true)]
	pub scope_subject: Option<String>,
	#[field]
	pub initialized: bool,
}

impl CoreRuns {}
