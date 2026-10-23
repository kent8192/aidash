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
	pub source_public_key_pem: Option<String>,
	#[field(field_type = "text", null = true)]
	pub source_key_id: Option<String>,
	#[field]
	pub atomic_transactions: bool,
	#[field]
	pub enabled: bool,
	#[field]
	pub revision: i64,
	#[field(field_type = "text")]
	pub actor: String,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field]
	pub updated_at: DateTime<Utc>,
}
