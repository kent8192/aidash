//! Persistent dashboard_login_transactions records.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "dashboard_login_transactions")]
#[derive(Serialize, Deserialize)]
pub struct DashboardLoginTransaction {
	#[field(primary_key = true)]
	pub state_hash: super::byte_key::ByteKey,
	#[field]
	pub browser_hash: Vec<u8>,
	#[field(field_type = "text")]
	pub nonce: String,
	#[field(field_type = "text")]
	pub pkce_verifier: String,
	#[field(field_type = "text")]
	pub return_to: String,
	#[field(field_type = "text")]
	pub callback_uri: String,
	#[field]
	pub expires_at: DateTime<Utc>,
	#[field(null = true)]
	pub started_at: Option<DateTime<Utc>>,
	#[field(field_type = "text", null = true)]
	pub gcip_tenant: Option<String>,
}
