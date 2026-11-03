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
	/// Historical source UUID; insertion checks same-Tenant existence.
	#[field]
	pub area_id: uuid::Uuid,
	#[field]
	pub sequence: i64,
	#[field]
	pub initialized: bool,
	#[field]
	pub generation: i64,
}

impl CoreRuns {}
