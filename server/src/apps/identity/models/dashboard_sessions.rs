//! Persistent dashboard_sessions records.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "dashboard_sessions")]
#[derive(Serialize, Deserialize)]
pub struct DashboardSession {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field]
	pub token_hash: Vec<u8>,
	#[field]
	pub csrf_hash: Vec<u8>,
	#[field]
	pub identity_id: uuid::Uuid,
	#[field(field_type = "text", null = true)]
	pub provider_sid: Option<String>,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field(null = true)]
	pub auth_time: Option<DateTime<Utc>>,
	#[field]
	pub last_activity_at: DateTime<Utc>,
	#[field]
	pub expires_at: DateTime<Utc>,
	#[field(null = true)]
	pub revoked_at: Option<DateTime<Utc>>,
	#[field(default = false)]
	pub desktop: bool,
	#[field(null = true)]
	pub desktop_idle_seconds: Option<i64>,
	#[field(null = true)]
	pub access_expires_at: Option<DateTime<Utc>>,
}

impl DashboardSession {}

crate::native_record!(DashboardSession {
	id,
	token_hash,
	csrf_hash,
	identity_id,
	provider_sid,
	created_at,
	auth_time,
	last_activity_at,
	expires_at,
	revoked_at,
	desktop,
	desktop_idle_seconds,
	access_expires_at
});
