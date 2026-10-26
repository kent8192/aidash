//! Persistent Preview records, one-time tickets and viewing sessions (ADR 0068).
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One random subdomain per viewable File version or Live preview.
#[model(app_label = "execution", table_name = "core_previews")]
#[derive(Serialize, Deserialize)]
pub struct CorePreviews {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	/// `file` or (later) `live`; the nullable ids carry the viewable thing.
	#[field(field_type = "text")]
	pub kind: String,
	#[field(null = true)]
	pub area_id: Option<Uuid>,
	#[field(null = true)]
	pub file_id: Option<Uuid>,
	#[field(null = true)]
	pub live_preview_id: Option<Uuid>,
	#[field(field_type = "text")]
	pub label: String,
	#[field(field_type = "text")]
	pub created_by: String,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

/// A one-time ticket digest; the secret itself never reaches the database.
#[model(app_label = "execution", table_name = "core_preview_tickets")]
#[derive(Serialize, Deserialize)]
pub struct CorePreviewTickets {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	pub preview_id: Uuid,
	#[field(field_type = "text")]
	pub subject: String,
	pub credential_id: Uuid,
	/// The dashboard origin the ticket's `frame-ancestors` names.
	#[field(field_type = "text")]
	pub origin: String,
	#[field(field_type = "text")]
	pub digest: String,
	pub expires_at: DateTime<Utc>,
	#[field(null = true)]
	pub used_at: Option<DateTime<Utc>>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

/// One viewing session: who viewed which Preview for how many bytes.
#[model(app_label = "execution", table_name = "core_preview_sessions")]
#[derive(Serialize, Deserialize)]
pub struct CorePreviewSessions {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	pub preview_id: Uuid,
	#[field(field_type = "text")]
	pub subject: String,
	pub credential_id: Uuid,
	#[field(field_type = "text")]
	pub origin: String,
	#[field(field_type = "text")]
	pub digest: String,
	#[field(default = 0)]
	pub served_bytes: i64,
	/// The cookie's `Max-Age`; a stolen value is useless after this instant.
	pub expires_at: DateTime<Utc>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	#[field(auto_now_add = true)]
	pub last_seen_at: DateTime<Utc>,
}
