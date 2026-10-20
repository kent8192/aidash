//! Persistent agent_plugin_copies records.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The Tenant's copy of one imported Agent Plugin (ADR 0072), addressed by the
/// tree digest an Agent definition pins. A row is never updated.
#[model(app_label = "registry", table_name = "agent_plugin_copies")]
#[derive(Serialize, Deserialize)]
pub struct AgentPluginCopy {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field(primary_key = true, field_type = "text")]
	pub digest: String,
	#[field(field_type = "text")]
	pub name: String,
	/// The commit-pinned GitHub URL of the plugin root.
	#[field(field_type = "text")]
	pub source: String,
	/// The serialized `LoadedPlugin`, written once from the import.
	#[field]
	pub plugin: Json<Value>,
	/// Every fetched file as `{path, encoding, content}`, sorted by path.
	#[field]
	pub files: Json<Value>,
	/// Paths the Agent Plugins specification forbids access to, left out of `files`.
	#[field]
	pub denied: Json<Value>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}
