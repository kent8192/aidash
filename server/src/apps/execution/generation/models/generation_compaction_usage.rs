//! Persistent generation_compaction_usage records.

use chrono::{DateTime, Utc};
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "generation_compaction_usage")]
#[derive(Serialize, Deserialize)]
pub struct GenerationCompactionUsage {
	#[field(primary_key = true, field_type = "uuid")]
	pub request_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(primary_key = true, field_type = "uuid")]
	pub attempt_id: uuid::Uuid,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "generation_compaction_usage_run_id_fkey",
		db_index = false
	)]
	pub run: ForeignKeyField<crate::apps::execution::models::Run>,
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(field_type = "text", db_column = "provider_id")]
	pub provider_key: String,
	#[field(field_type = "text")]
	pub provider_version: String,
	#[field]
	pub request_bytes: i64,
	#[field]
	pub questions: i32,
	#[field]
	pub created_at: DateTime<Utc>,
}

impl GenerationCompactionUsage {}
