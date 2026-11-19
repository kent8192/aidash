//! Duty request and response contracts (#1011, from the Schedules of #557).
use aidash_application::duties::Detail;
use aidash_domain::{
	approval_rules::ApprovalMode,
	duties::{
		Duty, Halt, Tick, Trigger,
		scripts::{Script, Version, Watcher},
	},
	registry::EntityRef,
};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A Duty as its list and page show it.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct DutyView {
	pub id: Uuid,
	/// The Duty's own Workspace, where its Tasks belong (#1008).
	pub workspace_id: Uuid,
	/// The Workspace it was registered from; None for a Duty from before #1008.
	pub origin_workspace_id: Option<Uuid>,
	/// The person every Run of the Duty acts as.
	pub requester: String,
	/// The viewer is its Requester.
	pub mine: bool,
	pub title: String,
	/// What the Duty is for.
	pub mission: String,
	/// The checklist its Agent follows on every Task.
	pub procedure: String,
	pub agent: EntityRef,
	/// When it wakes by the clock, if it does.
	pub trigger: Option<Trigger>,
	/// None follows the Requester's own default mode.
	pub approval_mode: Option<ApprovalMode>,
	pub max_lateness_seconds: i64,
	/// Failed Tasks in a row after which it pauses itself; 0 never does.
	pub pause_after_failures: i32,
	/// The script that decides what each event no Task booked does (#1011).
	pub router: Option<Script>,
	/// The script that polls an outside service on its own interval.
	pub watcher: Option<Watcher>,
	/// When the Watcher runs next.
	pub watcher_next_at: Option<DateTime<Utc>>,
	/// `active`, `paused` or `stopped`.
	pub status: String,
	pub halt: Option<Halt>,
	pub halted_at: Option<DateTime<Utc>>,
	/// Why it stopped, in words, when the halt alone does not say it all.
	pub halt_reason: Option<String>,
	pub successor: Option<String>,
	/// The next scheduled time; the viewer's time zone renders it.
	pub next_scheduled_at: Option<DateTime<Utc>>,
	pub consecutive_failures: i32,
	/// The Run whose Agent registered it, if an Agent did.
	pub created_by_run_id: Option<Uuid>,
	pub revision: i64,
	pub created_at: DateTime<Utc>,
	pub updated_at: DateTime<Utc>,
	/// What the viewer may do with it.
	pub may_edit: bool,
	/// Pause and delete: its Requester or a holder of `duty.administer`.
	pub may_manage: bool,
	/// Resume it: its Requester, or the successor named for it.
	pub may_resume: bool,
}

impl DutyView {
	pub fn new(duty: Duty, viewer: &str, administers: bool) -> Self {
		use aidash_domain::duties::{Actor, Change, permit, resumption};
		let actor = Actor {
			subject: viewer,
			administers,
		};
		let may = |change| permit(actor, duty.standing(), change).is_ok();
		let (may_edit, may_manage) = (may(Change::Edit), may(Change::Pause));
		let may_resume = resumption(actor, duty.standing()).is_ok();
		let status = duty.status().to_owned();
		let definition = duty.definition;
		Self {
			id: duty.id,
			workspace_id: duty.workspace_id,
			origin_workspace_id: duty.origin_workspace_id,
			mine: duty.requester == viewer,
			requester: duty.requester,
			title: definition.title,
			mission: definition.mission,
			procedure: definition.procedure,
			agent: definition.agent,
			trigger: definition.trigger,
			approval_mode: definition.approval_mode,
			max_lateness_seconds: definition.max_lateness_seconds,
			pause_after_failures: definition.pause_after_failures,
			router: definition.router,
			watcher: definition.watcher,
			watcher_next_at: duty.watcher_next_at,
			status,
			halt: duty.halt,
			halted_at: duty.halted_at,
			halt_reason: duty.halt_reason,
			successor: duty.successor,
			next_scheduled_at: duty.next_scheduled_at,
			consecutive_failures: duty.consecutive_failures,
			created_by_run_id: duty.created_by_run_id,
			revision: duty.revision,
			created_at: duty.created_at,
			updated_at: duty.updated_at,
			may_edit,
			may_manage,
			may_resume,
		}
	}
}

/// A Duty with its history and what the viewer may do.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct DetailView {
	pub duty: DutyView,
	/// Started, skipped (with the reason) and failed ticks, newest first.
	pub ticks: Vec<Tick>,
	/// A newer version of its Agent the Requester could move it to.
	pub newer_agent_version: Option<String>,
	/// Saved versions of its Router and Watcher with their authors, newest
	/// first (#1011).
	pub script_versions: Vec<Version>,
	pub may_edit: bool,
	pub may_administer: bool,
}

impl DetailView {
	pub fn new(detail: Detail, viewer: &str, script_versions: Vec<Version>) -> Self {
		Self {
			duty: DutyView::new(detail.duty, viewer, detail.may_administer),
			ticks: detail.ticks,
			newer_agent_version: detail.newer_agent_version,
			script_versions,
			may_edit: detail.may_edit,
			may_administer: detail.may_administer,
		}
	}
}

/// A Duty that stopped without anyone choosing it (#557 decision 25).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct DutyNotice {
	pub id: Uuid,
	pub workspace_id: Uuid,
	pub title: String,
	pub requester: String,
	pub halt: Halt,
	pub halted_at: Option<DateTime<Utc>>,
	pub halt_reason: Option<String>,
	pub successor: Option<String>,
}

impl DutyNotice {
	pub fn of(duty: &Duty) -> Self {
		Self {
			id: duty.id,
			workspace_id: duty.workspace_id,
			title: duty.definition.title.clone(),
			requester: duty.requester.clone(),
			halt: duty.halt.unwrap_or(Halt::Paused),
			halted_at: duty.halted_at,
			halt_reason: duty.halt_reason.clone(),
			successor: duty.successor.clone(),
		}
	}
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct NoticesView {
	pub stopped: Vec<DutyNotice>,
	/// Tasks the viewer's Duties started in the last week. The bell shows
	/// their completion without a toast (#557 decision 26).
	pub scheduled_tasks: Vec<Uuid>,
}

/// Which Duties `GET /api/duties` lists.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ListScope {
	/// The viewer's own Duties.
	#[default]
	Mine,
	/// Every Duty of the Tenant, for holders of `duty.administer`.
	Tenant,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DutyListQuery {
	#[serde(default)]
	pub scope: ListScope,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SuccessorInput {
	/// The person who may take the Duty over as their own.
	pub subject: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct RunNowView {
	pub task_id: Uuid,
	pub workspace_id: Uuid,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct DeletedView {
	pub deleted: Uuid,
}

/// An event another Node sends to a `node_event` Duty Trigger (#1013).
/// The sender is the Node its Federation token proves, never a field here.
#[derive(Debug, Clone, serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct NodeEventInput {
	pub trigger_id: Uuid,
	/// The event, JSON of at most 256 KiB, which the Duty reads as data.
	#[serde(default)]
	pub payload: serde_json::Value,
}

/// What a webhook request did (#1011).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct DutyEventReceipt {
	pub event_id: Uuid,
	pub outcome: aidash_domain::duties::events::EventOutcome,
}
