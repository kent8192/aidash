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
	/// The Duty's own Workspace (#1008, ADR 0140).
	#[field]
	pub workspace_id: Uuid,
	/// The Workspace it was registered from; None before #1008.
	#[field(null = true)]
	pub origin_workspace_id: Option<Uuid>,
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
	/// The Router script, if the Duty has one (#1011 D3).
	#[field(null = true)]
	pub router: Option<Json<Value>>,
	/// The Watcher script and its interval, if the Duty has one.
	#[field(null = true)]
	pub watcher: Option<Json<Value>>,
	/// When the Watcher runs next.
	#[field(null = true)]
	pub watcher_next_at: Option<DateTime<Utc>>,
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
	/// The person who clicked "Run now"; None for a scheduled tick.
	#[field(field_type = "text", null = true)]
	pub started_by: Option<String>,
	#[field(null = true)]
	pub settled_at: Option<DateTime<Utc>>,
}

/// A Duty Trigger other than the clock (#1011): a webhook, another Duty's
/// Task ending, or an event from another Node (#1013).
#[model(app_label = "workspaces", table_name = "duty_triggers")]
#[derive(Serialize, Deserialize)]
pub struct DutyTriggerRecord {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub workspace_id: Uuid,
	#[field]
	pub duty_id: Uuid,
	/// `webhook`, `duty_completed` or `node_event`.
	#[field(field_type = "text")]
	pub kind: String,
	/// The Duty whose Tasks ending wakes this one, for `duty_completed`.
	#[field(null = true)]
	pub source_duty_id: Option<Uuid>,
	/// The only Node a `node_event` trigger takes events from, if one is
	/// named (#1013).
	#[field(null = true, field_type = "text")]
	pub from_node: Option<String>,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field(field_type = "text")]
	pub created_by: String,
}

/// One event a Duty received (#1011). Its payload is data of the Duty's
/// Thread and goes with the Duty.
#[model(app_label = "workspaces", table_name = "duty_events")]
#[derive(Serialize, Deserialize)]
pub struct DutyEventRecord {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub workspace_id: Uuid,
	#[field]
	pub duty_id: Uuid,
	#[field]
	pub trigger_id: Uuid,
	#[field]
	pub received_at: DateTime<Utc>,
	#[field]
	pub payload: Json<Value>,
	/// `started`, `delivered`, `skipped`, `failed`, or with a Router
	/// `routing`, `dropped` or `acted`.
	#[field(field_type = "text")]
	pub outcome: String,
	#[field(field_type = "text", null = true)]
	pub reason: Option<String>,
	#[field(null = true)]
	pub task_id: Option<Uuid>,
	/// The Run a delivered event woke.
	#[field(null = true)]
	pub run_id: Option<Uuid>,
	/// What the Duty's Router decided about it (#1011 D3).
	#[field(null = true)]
	pub router: Option<Json<Value>>,
}

/// A Task's booking of the next event of a trigger (#1011): the event wakes
/// that Task's Run instead of starting a new Task.
#[model(app_label = "workspaces", table_name = "duty_bookings")]
#[derive(Serialize, Deserialize)]
pub struct DutyBookingRecord {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub workspace_id: Uuid,
	#[field]
	pub trigger_id: Uuid,
	#[field]
	pub task_id: Uuid,
	#[field]
	pub run_id: Uuid,
	#[field]
	pub created_at: DateTime<Utc>,
}

/// One saved version of a Duty's Router or Watcher, with who saved it and
/// when (#1011 D3). Keyed on the Duty and deleted with it.
#[model(app_label = "workspaces", table_name = "duty_script_versions")]
#[derive(Serialize, Deserialize)]
pub struct DutyScriptVersionRecord {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub duty_id: Uuid,
	/// `router` or `watcher`.
	#[field(field_type = "text")]
	pub role: String,
	/// `python` or `shell`.
	#[field(field_type = "text")]
	pub language: String,
	#[field(field_type = "text")]
	pub source: String,
	/// A Watcher's interval when this version was saved.
	#[field(null = true)]
	pub every_seconds: Option<i64>,
	#[field(field_type = "text")]
	pub author: String,
	#[field]
	pub created_at: DateTime<Utc>,
}

/// A Task a Duty started to run its Router or Watcher without a model
/// (#1011 D3): the frozen script, what it reads on stdin, and whether the
/// Duty has applied its outcome. Keyed on the Task.
#[model(app_label = "workspaces", table_name = "duty_script_runs")]
#[derive(Serialize, Deserialize)]
pub struct DutyScriptRunRecord {
	#[field(primary_key = true)]
	pub task_id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub duty_id: Uuid,
	/// `router` or `watcher`.
	#[field(field_type = "text")]
	pub role: String,
	/// The event a Router decides about.
	#[field(null = true)]
	pub event_id: Option<Uuid>,
	#[field(field_type = "text")]
	pub language: String,
	#[field(field_type = "text")]
	pub source: String,
	#[field(field_type = "text")]
	pub stdin: String,
	#[field]
	pub created_at: DateTime<Utc>,
	/// When the Duty applied what the script decided or reported.
	#[field(null = true)]
	pub settled_at: Option<DateTime<Utc>>,
}
