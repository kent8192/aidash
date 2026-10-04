//! Persistent dashboard_identities records.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "dashboard_identities")]
#[derive(Serialize, Deserialize)]
pub struct DashboardIdentity {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub issuer: String,
	#[field(field_type = "text")]
	pub subject: String,
	#[field(null = true)]
	pub last_valid_at: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub disabled_at: Option<DateTime<Utc>>,
}
