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
	/// `None` for a request made with a Live preview's session token.
	#[field(null = true)]
	pub operation_id: Option<Uuid>,
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
	/// 0 until the outcome is settled. A row that stays 0 was never settled, so
	/// its outcome is unknown: the process stopped or storage stayed unavailable.
	#[field]
	pub status: i32,
	#[field]
	pub sent_bytes: i64,
	#[field]
	pub received_bytes: i64,
	#[field]
	pub duration_ms: i64,
	/// The gateway's admission call. Null until the outcome is settled.
	#[field(null = true)]
	pub admission_ms: Option<i64>,
	/// From the end of the response until its final debit settled. Null until
	/// the outcome is settled.
	#[field(null = true)]
	pub settle_ms: Option<i64>,
	/// `operation`, `pod` or `page`: which path admitted the request.
	#[field(field_type = "text")]
	pub via: String,
	#[field(field_type = "text", null = true)]
	pub error: Option<String>,
	/// The Tenant Secret and pinned version a call carried, and the host it
	/// went to. All null for a service without a Tenant Secret (ADR 0095).
	#[field(null = true)]
	pub secret_id: Option<Uuid>,
	#[field(field_type = "text", null = true)]
	pub secret_version: Option<String>,
	#[field(field_type = "text", null = true)]
	pub secret_host: Option<String>,
	#[field]
	pub created_at: DateTime<Utc>,
}

/// Durable suspension and connection-rate state for a Scratch Space.
#[model(app_label = "execution", table_name = "egress_scope_state")]
#[derive(Serialize, Deserialize)]
pub struct ScopeState {
	#[field(primary_key = true)]
	pub scope_id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub node: String,
	#[field]
	pub suspended: bool,
	#[field]
	pub window_started_at: DateTime<Utc>,
	#[field]
	pub connections_in_window: i32,
}

/// Scope totals use an empty service; other rows hold a service's daily limit.
#[model(app_label = "execution", table_name = "egress_scope_daily")]
#[derive(Serialize, Deserialize)]
pub struct ScopeDaily {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub node: String,
	#[field]
	pub scope_id: Uuid,
	#[field(field_type = "text")]
	pub day: String,
	#[field(field_type = "text")]
	pub service: String,
	#[field]
	pub sent_bytes: i64,
	#[field]
	pub received_bytes: i64,
}

/// Metadata only: request paths and payloads are deliberately absent.
#[model(app_label = "execution", table_name = "egress_scope_connections")]
#[derive(Serialize, Deserialize)]
pub struct ScopeConnection {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub node: String,
	#[field]
	pub scope_id: Uuid,
	#[field(field_type = "text")]
	pub host: String,
	#[field]
	pub port: i32,
	#[field]
	pub sent_bytes: i64,
	#[field]
	pub received_bytes: i64,
	#[field]
	pub duration_ms: i64,
	#[field(field_type = "text")]
	pub outcome: String,
	/// The code the client received. Null for a connection that completed or
	/// is still open, and for rows older than #845.
	#[field(field_type = "text", null = true)]
	pub error: Option<String>,
	/// How long the connection waited for a Host connection limit. Null until
	/// the wait is over, and for rows older than #845.
	#[field(null = true)]
	pub wait_ms: Option<i64>,
	#[field]
	pub created_at: DateTime<Utc>,
}

/// Notification outbox: a stop and its event are committed together.
#[model(app_label = "execution", table_name = "egress_scope_events")]
#[derive(Serialize, Deserialize)]
pub struct ScopeEvent {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub node: String,
	#[field]
	pub scope_id: Uuid,
	#[field(field_type = "text")]
	pub kind: String,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field(null = true)]
	pub delivered_at: Option<DateTime<Utc>>,
}
