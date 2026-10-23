//! Persistent generation_history records.

use chrono::{DateTime, Utc};
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "generation_history")]
#[derive(Serialize, Deserialize)]
pub struct GenerationHistory {
	#[field(primary_key = true)]
	pub sequence: i64,
	#[field(field_type = "text")]
	pub tenant: String,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "generation_history_request_id_fkey",
		db_index = false
	)]
	pub request: ForeignKeyField<crate::apps::execution::generation::models::GenerationRequest>,
	#[field(field_type = "text")]
	pub status: String,
	#[field(field_type = "text")]
	pub actor: String,
	#[field(field_type = "text")]
	pub reason: String,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

impl GenerationHistory {}
