//! Persistent core_runs records.
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "core_runs")]
#[derive(Serialize, Deserialize)]
pub struct CoreRuns {
	#[field(primary_key = true)]
	pub run_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "core_runs_area_id_fkey",
		db_index = false
	)]
	pub area: ForeignKeyField<crate::apps::execution::capabilities::models::CoreAreas>,
	#[field]
	pub sequence: i64,
	#[field]
	pub initialized: bool,
	#[field]
	pub generation: i64,
}

impl CoreRuns {}
