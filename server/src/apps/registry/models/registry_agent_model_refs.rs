//! Persistent registry_agent_model_refs records.

use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "registry", table_name = "registry_agent_model_refs")]
#[derive(Serialize, Deserialize)]
pub struct RegistryAgentModelRef {
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(primary_key = true, field_type = "text", db_column = "agent_id")]
	pub agent_key: String,
	#[field(primary_key = true, field_type = "text")]
	pub agent_version: String,
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(field_type = "text", db_column = "model_id")]
	pub model_key: String,
	#[field(field_type = "text")]
	pub model_version: String,
	#[field(field_type = "text")]
	pub model_kind: String,
}
