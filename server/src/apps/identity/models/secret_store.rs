//! Encrypted Store resources support namespaces independent of provider metadata.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};
#[model(app_label = "identity", table_name = "credential_store_resources")]
#[derive(Serialize, Deserialize)]
pub struct Resource {
	#[field(primary_key = true, field_type = "text")]
	pub resource: String,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub next_version: i64,
	#[field]
	pub created_at: DateTime<Utc>,
}
#[model(app_label = "identity", table_name = "credential_store_versions")]
#[derive(Serialize, Deserialize)]
pub struct Version {
	#[field(primary_key = true, field_type = "text")]
	pub resource: String,
	// Allocated from the resource's next_version counter, never a database sequence.
	#[field(primary_key = true, auto_increment = false)]
	pub version: i64,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub key_id: String,
	#[field(field_type = "text")]
	pub algorithm: String,
	#[field]
	pub nonce: Vec<u8>,
	#[field]
	pub ciphertext: Vec<u8>,
	#[field(field_type = "text")]
	pub state: String,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field(null = true)]
	pub disabled_at: Option<DateTime<Utc>>,
}
#[model(app_label = "identity", table_name = "credential_store_keys")]
#[derive(Serialize, Deserialize)]
pub struct RegisteredKey {
	#[field(primary_key = true, field_type = "text")]
	pub key_id: String,
	#[field]
	pub check_nonce: Vec<u8>,
	#[field]
	pub check_ciphertext: Vec<u8>,
	#[field]
	pub created_at: DateTime<Utc>,
}
