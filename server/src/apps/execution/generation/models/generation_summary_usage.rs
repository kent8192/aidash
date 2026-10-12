//! Persistent generation_summary_usage records.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "generation_summary_usage")]
#[derive(Serialize, Deserialize)]
pub struct GenerationSummaryUsage {
	#[field(primary_key = true, field_type = "uuid")]
	pub request_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "uuid")]
	pub attempt_id: uuid::Uuid,
	#[field]
	pub run_id: uuid::Uuid,
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(field_type = "text", db_column = "provider_id")]
	pub provider_key: String,
	#[field(field_type = "text")]
	pub provider_version: String,
	#[field(field_type = "text")]
	pub definition_digest: String,
	#[field]
	pub request_bytes: i64,
	#[field]
	pub created_at: DateTime<Utc>,
}

impl GenerationSummaryUsage {}
