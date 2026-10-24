//! Persistent generation_usage records.

use chrono::{DateTime, Utc};
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "generation_usage")]
#[derive(Serialize, Deserialize)]
pub struct GenerationUsage {
	#[field(primary_key = true, field_type = "uuid")]
	pub request_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(primary_key = true, field_type = "uuid")]
	pub attempt_id: uuid::Uuid,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "generation_usage_run_id_fkey",
		db_index = false
	)]
	pub run: ForeignKeyField<crate::apps::execution::models::Run>,
	#[field]
	pub reserved_tokens: i64,
	#[field(null = true)]
	pub reported_tokens: Option<i64>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

impl GenerationUsage {}
