//! Original reference image bytes, private to the owning knowledge row.
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "registry", table_name = "agent_knowledge_images")]
#[derive(Serialize, Deserialize)]
pub struct AgentKnowledgeImage {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field(primary_key = true, field_type = "text", db_column = "agent_id")]
	pub agent_key: String,
	#[field(primary_key = true, field_type = "text")]
	pub agent_version: String,
	#[field(primary_key = true, auto_increment = false)]
	pub document_index: i32,
	#[field(field_type = "text")]
	pub media_type: String,
	#[field]
	pub size: i64,
	#[field(field_type = "text")]
	pub sha256: String,
	#[field]
	pub bytes: Vec<u8>,
}
