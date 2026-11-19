//! Every revision of a Node Registration, in order (#241).
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "federation", table_name = "node_registration_history")]
#[derive(Serialize, Deserialize)]
pub struct NodeRegistrationHistory {
	#[field(primary_key = true)]
	pub sequence: i64,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub node: String,
	#[field(field_type = "text")]
	pub source_node: String,
	#[field(field_type = "text", null = true)]
	pub source_endpoint: Option<String>,
	#[field(field_type = "text", null = true)]
	pub source_dashboard_origin: Option<String>,
	#[field(field_type = "text", null = true)]
	pub source_public_key_pem: Option<String>,
	#[field(field_type = "text", null = true)]
	pub source_key_id: Option<String>,
	/// Receiver-local current/retiring/quarantined authentication state.
	#[field(field_type = "text", null = true)]
	pub source_keys: Option<String>,
	/// Authentication generation; independent of authorization revision.
	#[field(default = 1)]
	pub keys_revision: i64,
	#[field]
	pub atomic_transactions: bool,
	#[field]
	pub enabled: bool,
	/// A Federation Tool Request; absent means the default one (ADR 0139).
	#[field]
	pub tool_request: Option<reinhardt::db::orm::Json<serde_json::Value>>,
	/// A Federation Tool Allowance; absent means the default list.
	#[field]
	pub tool_allowance: Option<reinhardt::db::orm::Json<serde_json::Value>>,
	/// A Federation Tool Block; empty by default.
	#[field]
	pub tool_block: reinhardt::db::orm::Json<serde_json::Value>,
	#[field]
	pub revision: i64,
	#[field(field_type = "text")]
	pub actor: String,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field]
	pub updated_at: DateTime<Utc>,
}
