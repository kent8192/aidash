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
	#[field]
	pub last_activity_at: DateTime<Utc>,
	#[field]
	pub expires_at: DateTime<Utc>,
	#[field(null = true)]
	pub revoked_at: Option<DateTime<Utc>>,
}

impl DashboardSession {}
