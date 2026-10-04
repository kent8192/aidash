//! Persistent semantic_agent_memory records.

use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "knowledge", table_name = "semantic_agent_memory")]
#[derive(Serialize, Deserialize)]
pub struct SemanticAgentMemory {
	#[field(primary_key = true)]
	pub entry_id: uuid::Uuid,
	#[field]
	pub workspace_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub agent_id: String,
	#[field(field_type = "text")]
	pub agent_version: String,
	#[field(field_type = "text")]
	pub home_node: String,
}

impl SemanticAgentMemory {}
