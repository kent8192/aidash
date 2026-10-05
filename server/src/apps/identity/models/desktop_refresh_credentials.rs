//! Persistent desktop_refresh_credentials records.
use reinhardt::model;
use serde::{Deserialize, Serialize};
#[model(app_label = "identity", table_name = "desktop_refresh_credentials")]
#[derive(Serialize, Deserialize)]
pub struct DesktopRefreshCredential {
	#[field(primary_key = true)]
	pub token_hash: super::byte_key::ByteKey,
	#[field]
	pub session_id: uuid::Uuid,
	#[field(null = true)]
	pub next_hash: Option<Vec<u8>>,
}
