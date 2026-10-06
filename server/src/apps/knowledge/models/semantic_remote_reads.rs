//! Durable semantic_remote_reads rows.
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "knowledge", table_name = "semantic_remote_reads")]
#[derive(Serialize, Deserialize)]
pub struct SemanticRemoteReads {
	#[field(primary_key = true, field_type = "uuid")]
	pub grant_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "uuid")]
	pub admission_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "uuid")]
	pub entry_id: uuid::Uuid,
	#[field(primary_key = true)]
	pub revision: i64,
	#[field(field_type = "text")]
	pub content_digest: String,
}
