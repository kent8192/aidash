//! Durable operation budgets and metadata-only gateway request records.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "execution", table_name = "egress_budgets")]
#[derive(Serialize, Deserialize)]
pub struct Budget {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub operation_id: Uuid,
	/// Empty for the operation total; otherwise the bound External service ID.
	#[field(field_type = "text")]
	pub service: String,
	#[field]
	pub expires_at: DateTime<Utc>,
	/// The Runner's actual command deadline, pinned at its first request.
	#[field(null = true)]
	pub deadline_at: Option<DateTime<Utc>>,
	#[field]
	pub sent_bytes: i64,
	#[field]
	pub received_bytes: i64,
	#[field]
	pub requests: i32,
	#[field]
	pub max_sent_bytes: i64,
	#[field]
	pub max_received_bytes: i64,
	#[field]
	pub max_requests: i32,
}

#[model(app_label = "execution", table_name = "egress_requests")]
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "EgressRequest")]
pub struct RequestRecord {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub run_id: Uuid,
	#[field]
	pub operation_id: Uuid,
	/// Set only for a request made with a Live preview's session token.
	#[field(null = true)]
	pub session_id: Option<Uuid>,
	#[field(field_type = "text")]
	pub service: String,
	#[field(field_type = "text")]
	pub method: String,
	#[field(field_type = "text")]
	pub path: String,
	#[field(field_type = "text")]
	pub query_digest: String,
	#[field]
	pub status: i32,
	#[field]
	pub sent_bytes: i64,
	#[field]
	pub received_bytes: i64,
	#[field]
	pub duration_ms: i64,
	#[field(field_type = "text", null = true)]
	pub error: Option<String>,
	#[field]
	pub created_at: DateTime<Utc>,
}
