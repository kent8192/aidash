//! A Live preview's persisted facts, a Tenant's lowered limits and the
//! session-lifetime Egress budget its Web server holds (ADR 0070, ADR 0069).
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One Run's Live preview. `state` carries `starting`, `running`, `stale` or
/// `stopped`; `stop_reason` explains a stop and stays `NULL` while it serves.
#[model(app_label = "execution", table_name = "core_live_previews")]
#[derive(Serialize, Deserialize)]
pub struct CoreLivePreviews {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	pub run_id: Uuid,
	pub area_id: Uuid,
	pub revision: i64,
	pub port: i32,
	#[field(field_type = "text")]
	pub state: String,
	#[field(field_type = "text", null = true)]
	pub stop_reason: Option<String>,
	pub idle_seconds: i64,
	pub absolute_seconds: i64,
	pub started_at: DateTime<Utc>,
	pub last_request_at: DateTime<Utc>,
	pub expires_at: DateTime<Utc>,
	/// The Pod address the Preview gateway proxies to; never shown to a model.
	#[field(field_type = "text", null = true)]
	pub pod_host: Option<String>,
	#[field(default = true)]
	pub termination_confirmed: bool,
	pub egress_expires_at: Option<DateTime<Utc>>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	#[field(auto_now_add = true)]
	pub updated_at: DateTime<Utc>,
}

/// A Tenant's lowered Live preview limits; absent means the ceilings apply.
#[model(app_label = "execution", table_name = "core_preview_limits")]
#[derive(Serialize, Deserialize)]
pub struct CorePreviewLimits {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	pub idle_seconds: i64,
	pub absolute_seconds: i64,
	pub revision: i64,
	#[field(auto_now_add = true)]
	pub updated_at: DateTime<Utc>,
}

/// The Egress budget of a Live preview's session-lifetime token. It ends with
/// the preview, not with an operation.
#[model(app_label = "execution", table_name = "egress_sessions")]
#[derive(Serialize, Deserialize)]
pub struct EgressSession {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	pub run_id: Uuid,
	pub live_preview_id: Uuid,
	/// The subject the ticket-issuing viewer acted as when the token was minted.
	#[field(field_type = "text")]
	pub principal: String,
	pub credential_id: Uuid,
	pub expires_at: DateTime<Utc>,
	#[field(default = 0)]
	pub sent_bytes: i64,
	#[field(default = 0)]
	pub received_bytes: i64,
	#[field(default = 0)]
	pub requests: i32,
	pub max_sent_bytes: i64,
	pub max_received_bytes: i64,
	pub max_requests: i32,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}
