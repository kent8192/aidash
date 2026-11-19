//! Tenant-owned Duty records (#1011, ADR 0142; from the Schedules of #557).
use chrono::{DateTime, Utc};
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[model(app_label = "workspaces", table_name = "duties")]
#[derive(Serialize, Deserialize)]
pub struct DutyRecord {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub workspace_id: Uuid,
	/// The person every Run of the Duty acts as.
	#[field(field_type = "text")]
	pub requester: String,
	#[field]
	pub credential_id: Uuid,
	#[field(field_type = "text")]
	pub title: String,
	/// What the Duty is for.
	#[field(field_type = "text")]
	pub mission: String,
	/// The checklist its Agent follows on every Task.
	#[field(field_type = "text")]
	pub procedure: String,
	#[field(field_type = "text")]
	pub agent_id: String,
	#[field(field_type = "text")]
	pub agent_version: String,
	/// The time trigger; None for a Duty that wakes only otherwise.
	#[field(null = true)]
	pub trigger: Option<Json<Value>>,
	/// The Duty's own Thread, once its first Task opened it.
	#[field(null = true)]
	pub thread_id: Option<Uuid>,
	#[field(field_type = "text", null = true)]
	pub approval_mode: Option<String>,
	#[field]
	pub max_lateness_seconds: i64,
	/// Failed Tasks in a row after which it pauses itself; 0 never does.
	#[field]
	pub pause_after_failures: i32,
	#[field]
	pub anchor: DateTime<Utc>,
	#[field(null = true)]
	pub next_scheduled_at: Option<DateTime<Utc>>,
	/// The next scheduled time plus the Duty's hidden jitter.
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

#[model(app_label = "workspaces", table_name = "duty_ticks")]
#[derive(Serialize, Deserialize)]
pub struct DutyTickRecord {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub duty_id: Uuid,
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
