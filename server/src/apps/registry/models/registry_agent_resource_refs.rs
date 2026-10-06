//! Persistent registry_agent_resource_refs records.

use crate::apps::registry::services::states::RegistryAgentResourceRefRequiredKind;
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "registry", table_name = "registry_agent_resource_refs")]
#[derive(Serialize, Deserialize)]
pub struct RegistryAgentResourceRef {
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(primary_key = true, field_type = "text", db_column = "agent_id")]
	pub agent_key: String,
	#[field(primary_key = true, field_type = "text")]
	pub agent_version: String,
	#[field(primary_key = true, field_type = "text", max_length = 64)]
	pub required_kind: RegistryAgentResourceRefRequiredKind,
	#[field(primary_key = true)]
	pub ordinal: i32,
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(field_type = "text", db_column = "reference_id")]
	pub reference_key: String,
	#[field(field_type = "text")]
	pub reference_version: String,
}
