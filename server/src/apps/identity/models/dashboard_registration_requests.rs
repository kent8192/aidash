//! Persistent dashboard_registration_requests records.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "dashboard_registration_requests")]
#[derive(Serialize, Deserialize)]
pub struct DashboardRegistrationRequest {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field]
	pub identity_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub status: String,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field]
	pub expires_at: DateTime<Utc>,
	#[field(null = true)]
	pub decided_at: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub decided_by: Option<uuid::Uuid>,
	#[field(field_type = "text", null = true)]
	pub decision_actor: Option<String>,
}

impl DashboardRegistrationRequest {}
