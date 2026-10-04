//! Persistent generation_embedding_usage records.

use crate::apps::execution::generation::services::states::GenerationEmbeddingUsagePurpose;
use chrono::{DateTime, Utc};
use reinhardt::macros::Model;
use serde::{Deserialize, Serialize};

#[derive(Model, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[model_config(app_label = "execution", table_name = "generation_embedding_usage")]
pub struct GenerationEmbeddingUsage {
	#[field(primary_key = true, db_column = "request_id", field_type = "uuid")]
	pub request_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "uuid")]
	pub attempt_id: uuid::Uuid,

	#[field(db_column = "workspace_id")]
	pub workspace_id: uuid::Uuid,

	#[field(db_column = "run_id", null = true)]
	pub run_id: Option<uuid::Uuid>,
	#[field(null = true)]
	pub entry_id: Option<uuid::Uuid>,
	#[field(field_type = "text", max_length = 64)]
	pub purpose: GenerationEmbeddingUsagePurpose,
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(field_type = "text", db_column = "provider_id")]
	pub provider_key: String,
	#[field(field_type = "text")]
	pub provider_version: String,
	#[field]
	pub request_bytes: i64,
	#[field]
	pub reserved_tokens: i64,
	#[field(null = true)]
	pub reported_tokens: Option<i64>,
	#[field]
	pub created_at: DateTime<Utc>,
}
