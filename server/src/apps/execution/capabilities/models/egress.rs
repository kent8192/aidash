//! Metadata-only gateway request records and Scratch Space egress state.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
	/// `operation`, `pod` or `page`: which path admitted the request. New
	/// records are `pod`; the others remain on records from the removed routes.
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

/// One row per Scratch Space that has used the gateway. Locking it serializes
/// the scope's budget charges, and the scope's other rows hang off it.
#[model(app_label = "execution", table_name = "egress_scope_state")]
#[derive(Serialize, Deserialize)]
pub struct ScopeState {
	#[field(primary_key = true)]
	pub scope_id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub node: String,
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

/// A committed Host reservation, written in the same transaction as the debit.
#[model(app_label = "execution", table_name = "egress_scope_host_debits")]
#[derive(Serialize, Deserialize)]
pub(crate) struct ScopeHostDebit {
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
	#[field]
	pub sent: bool,
	#[field]
	pub bytes: i64,
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
	#[field(null = true)]
	pub run_id: Option<Uuid>,
	#[field(null = true)]
	pub admission_ms: Option<i64>,
	#[field(null = true)]
	pub settle_ms: Option<i64>,
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

/// Counts of a scope's connections that ended before they held a Host
/// connection limit, per minute, host and reason (#845). One gateway instance
/// writes each minute once, so a minute may have a row per instance.
#[model(app_label = "execution", table_name = "egress_scope_refusals")]
#[derive(Serialize, Deserialize)]
pub struct ScopeRefusal {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub node: String,
	#[field]
	pub scope_id: Uuid,
	#[field]
	pub minute: DateTime<Utc>,
	/// Empty for the hosts past the per-minute cap, counted together.
	#[field(field_type = "text")]
	pub host: String,
	#[field(field_type = "text")]
	pub reason: String,
	#[field]
	pub count: i64,
}

/// Notifications for the operator, such as a scope reaching many hosts.
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

/// A Run whose read authority has been revoked.
#[model(app_label = "execution", table_name = "egress_run_revocations")]
#[derive(Serialize, Deserialize)]
pub struct RunRevocation {
	#[field(primary_key = true)]
	pub run_id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub revoked_at: DateTime<Utc>,
	#[field(field_type = "text")]
	pub reason: String,
	#[field(null = true)]
	pub stopped_at: Option<DateTime<Utc>>,
}
