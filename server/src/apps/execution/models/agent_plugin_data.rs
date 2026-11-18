//! The saved `${PLUGIN_DATA}` of an Agent's Agent Plugin (ADR 0073), one current version per
//! Tenant, Scratch Space, Agent and plugin.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "execution", table_name = "agent_plugin_data")]
#[derive(Serialize, Deserialize)]
pub struct AgentPluginData {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	/// The Scratch Space whose Pod the data lives in; everyone who may use the
	/// Space already shares that Pod.
	#[field]
	pub scratch_space_id: Uuid,
	#[field(field_type = "text")]
	pub agent_id: String,
	#[field(field_type = "text")]
	pub plugin: String,
	/// The files, packed by `aidash_domain::agent_plugin::stdio_session::pack_data`.
	#[field]
	pub data: Vec<u8>,
	/// The files' combined bytes.
	#[field]
	pub size: i64,
	#[field]
	pub files: i32,
	/// Hex SHA-256 of `data`.
	#[field(field_type = "text")]
	pub sha256: String,
	#[field]
	pub saved_at: DateTime<Utc>,
}
