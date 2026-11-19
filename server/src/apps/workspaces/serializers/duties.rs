//! Schedule request and response contracts (#557).
use aidash_application::schedules::Detail;
use aidash_domain::{
	approval_rules::ApprovalMode,
	registry::EntityRef,
	schedules::{Ceilings, Halt, Limits, Schedule, ThreadMode, Tick, Trigger},
};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A Schedule as its list and page show it.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct ScheduleView {
	pub id: Uuid,
	pub workspace_id: Uuid,
	/// The person every Run of the Schedule acts as.
	pub registrant: String,
	/// The viewer is its Registrant.
	pub mine: bool,
	pub title: String,
	pub request: String,
	pub agent: EntityRef,
	pub trigger: Trigger,
	pub thread: ThreadMode,
	/// None follows the Registrant's own default mode.
	pub approval_mode: Option<ApprovalMode>,
	pub max_lateness_seconds: i64,
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
	/// Pause and delete: its Registrant or a holder of `schedule.administer`.
	pub may_manage: bool,
	/// Resume it: its Registrant, or the successor named for it.
	pub may_resume: bool,
}

impl ScheduleView {
	pub fn new(schedule: Schedule, viewer: &str, administers: bool) -> Self {
		use aidash_domain::schedules::{Change, Requester, permit, resumption};
		let requester = Requester {
			subject: viewer,
			administers,
		};
		let may = |change| permit(requester, schedule.standing(), change).is_ok();
		let (may_edit, may_manage) = (may(Change::Edit), may(Change::Pause));
		let may_resume = resumption(requester, schedule.standing()).is_ok();
		let status = schedule.status().to_owned();
		let definition = schedule.definition;
		Self {
			id: schedule.id,
			workspace_id: schedule.workspace_id,
			mine: schedule.registrant == viewer,
			registrant: schedule.registrant,
			title: definition.title,
			request: definition.request,
			agent: definition.agent,
			trigger: definition.trigger,
			thread: definition.thread,
			approval_mode: definition.approval_mode,
			max_lateness_seconds: definition.max_lateness_seconds,
			status,
			halt: schedule.halt,
			halted_at: schedule.halted_at,
			halt_reason: schedule.halt_reason,
			successor: schedule.successor,
			next_scheduled_at: schedule.next_scheduled_at,
			consecutive_failures: schedule.consecutive_failures,
			created_by_run_id: schedule.created_by_run_id,
			revision: schedule.revision,
			created_at: schedule.created_at,
			updated_at: schedule.updated_at,
			may_edit,
			may_manage,
			may_resume,
		}
	}
}

/// A Schedule with its history and what the viewer may do.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct DetailView {
	pub schedule: ScheduleView,
	/// Started, skipped (with the reason) and failed ticks, newest first.
	pub ticks: Vec<Tick>,
	/// A newer version of its Agent the Registrant could move it to.
	pub newer_agent_version: Option<String>,
	pub may_edit: bool,
	pub may_administer: bool,
}

impl DetailView {
	pub fn new(detail: Detail, viewer: &str) -> Self {
		Self {
			schedule: ScheduleView::new(detail.schedule, viewer, detail.may_administer),
			ticks: detail.ticks,
			newer_agent_version: detail.newer_agent_version,
			may_edit: detail.may_edit,
			may_administer: detail.may_administer,
		}
	}
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct LimitsView {
	/// The Tenant's limits as its administrators set them.
	pub limits: Limits,
	pub ceilings: Ceilings,
	/// The limits that apply now, after the ceilings and any Tier.
	pub effective: Limits,
}

/// A Schedule that stopped without anyone choosing it (#557 decision 25).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct ScheduleNotice {
	pub id: Uuid,
	pub workspace_id: Uuid,
	pub title: String,
	pub registrant: String,
	pub halt: Halt,
	pub halted_at: Option<DateTime<Utc>>,
	pub halt_reason: Option<String>,
	pub successor: Option<String>,
}

impl ScheduleNotice {
	pub fn of(schedule: &Schedule) -> Self {
		Self {
			id: schedule.id,
			workspace_id: schedule.workspace_id,
			title: schedule.definition.title.clone(),
			registrant: schedule.registrant.clone(),
			halt: schedule.halt.unwrap_or(Halt::Paused),
			halted_at: schedule.halted_at,
			halt_reason: schedule.halt_reason.clone(),
			successor: schedule.successor.clone(),
		}
	}
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct NoticesView {
	pub stopped: Vec<ScheduleNotice>,
	/// Tasks the viewer's Schedules started in the last week. The bell shows
	/// their completion without a toast (#557 decision 26).
	pub scheduled_tasks: Vec<Uuid>,
}

/// Which Schedules `GET /api/schedules` lists.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ListScope {
	/// The viewer's own Schedules.
	#[default]
	Mine,
	/// Every Schedule of the Tenant, for holders of `schedule.administer`.
	Tenant,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScheduleListQuery {
	#[serde(default)]
	pub scope: ListScope,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SuccessorInput {
	/// The person who may take the Schedule over as their own.
	pub subject: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct RunNowView {
	pub task_id: Uuid,
	pub thread_id: Uuid,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct DeletedView {
	pub deleted: Uuid,
}
