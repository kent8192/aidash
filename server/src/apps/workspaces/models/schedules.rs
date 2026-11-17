//! Tenant-owned Schedule records (#557, ADR 0134).
use chrono::{DateTime, Utc};
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[model(app_label = "workspaces", table_name = "schedules")]
#[derive(Serialize, Deserialize)]
pub struct ScheduleRecord {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub workspace_id: Uuid,
	/// The person every Run of the Schedule acts as.
	#[field(field_type = "text")]
	pub registrant: String,
	#[field]
	pub credential_id: Uuid,
	#[field(field_type = "text")]
	pub title: String,
	#[field(field_type = "text")]
	pub request: String,
	#[field(field_type = "text")]
	pub agent_id: String,
	#[field(field_type = "text")]
	pub agent_version: String,
	#[field]
	pub trigger: Json<Value>,
	/// The fixed Thread; None opens a new Thread on every tick.
	#[field(null = true)]
	pub thread_id: Option<Uuid>,
	#[field(field_type = "text", null = true)]
	pub approval_mode: Option<String>,
	#[field]
	pub max_lateness_seconds: i64,
	#[field]
	pub anchor: DateTime<Utc>,
	#[field(null = true)]
	pub next_scheduled_at: Option<DateTime<Utc>>,
	/// The next scheduled time plus the Schedule's hidden jitter.
	#[field(null = true)]
	pub next_fire_at: Option<DateTime<Utc>>,
	#[field(field_type = "text", null = true)]
	pub halt: Option<String>,
	#[field(null = true)]
	pub halted_at: Option<DateTime<Utc>>,
	#[field(field_type = "text", null = true)]
	pub halt_reason: Option<String>,
	#[field(field_type = "text", null = true)]
	pub successor: Option<String>,
	#[field(default = 0)]
	pub consecutive_failures: i32,
	#[field(null = true)]
	pub created_by_run_id: Option<Uuid>,
	/// The invocation key of the Agent tool call that registered it.
	#[field(field_type = "text", null = true)]
	pub creation_key: Option<String>,
	#[field(default = 0)]
	pub revision: i64,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field]
	pub updated_at: DateTime<Utc>,
}

#[model(app_label = "workspaces", table_name = "schedule_ticks")]
#[derive(Serialize, Deserialize)]
pub struct ScheduleTickRecord {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub schedule_id: Uuid,
	#[field]
	pub workspace_id: Uuid,
	#[field]
	pub scheduled_at: DateTime<Utc>,
	#[field]
	pub recorded_at: DateTime<Utc>,
	#[field(field_type = "text")]
	pub status: String,
	#[field(field_type = "text", null = true)]
	pub reason: Option<String>,
	#[field(null = true)]
	pub task_id: Option<Uuid>,
	#[field(null = true)]
	pub thread_id: Option<Uuid>,
	/// The person who clicked "Run now"; None for a scheduled tick.
	#[field(field_type = "text", null = true)]
	pub started_by: Option<String>,
	#[field(null = true)]
	pub settled_at: Option<DateTime<Utc>>,
}

#[model(app_label = "workspaces", table_name = "schedule_limits")]
#[derive(Serialize, Deserialize)]
pub struct ScheduleLimitRecord {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field]
	pub max_schedules: i32,
	#[field]
	pub max_per_person: i32,
	#[field]
	pub min_interval_seconds: i64,
	#[field(default = 0)]
	pub revision: i64,
	#[field]
	pub updated_at: DateTime<Utc>,
	#[field(field_type = "text")]
	pub updated_by: String,
}
