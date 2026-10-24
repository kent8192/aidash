//! Persistent dashboard_logout_tokens records.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "dashboard_logout_tokens")]
#[derive(Serialize, Deserialize)]
pub struct DashboardLogoutToken {
	#[field(primary_key = true)]
	pub jti_hash: super::byte_key::ByteKey,
	#[field]
	pub expires_at: DateTime<Utc>,
}
