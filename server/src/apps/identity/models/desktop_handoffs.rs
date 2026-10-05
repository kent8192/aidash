//! Persistent desktop_handoffs records.
use reinhardt::model;
use serde::{Deserialize, Serialize};
#[model(app_label = "identity", table_name = "desktop_handoffs")]
#[derive(Serialize, Deserialize)]
pub struct DesktopHandoff {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub state: String,
	#[field(field_type = "text")]
	pub challenge: String,
	#[field(field_type = "text")]
	pub redirect_uri: String,
	#[field(field_type = "text")]
	pub origin: String,
	#[field(null = true)]
	pub browser_session_id: Option<uuid::Uuid>,
	#[field(null = true, unique = true)]
	pub code_hash: Option<Vec<u8>>,
	#[field]
	pub expires_at: chrono::DateTime<chrono::Utc>,
}
