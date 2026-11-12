//! Schedule use cases (#557, ADR 0134). The scope owns one transaction and
//! acts for one person: a person managing Schedules in the dashboard, or the
//! person at the root of an Agent's Run when an Agent calls a Schedule tool.
//! A Schedule registered here acts as that person, its Registrant.
use crate::{Error, Result};
use aidash_domain::{
	approval_rules::ApprovalMode,
	registry::EntityRef,
	schedules::{
		self, Ceilings, Change, Definition, Halt, Limits, Patch, Requester, Resumption, Schedule,
		ThreadMode, Tick,
	},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Which Schedules a list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listing {
	/// Every Schedule of one Workspace the person can see.
	Workspace(Uuid),
	/// The person's own Schedules, in every Workspace.
	Mine,
	/// Every Schedule of the Tenant, for holders of `schedule.administer`.
	Tenant,
}

/// Persistence and policy for one person's Schedule transaction.
#[async_trait]
pub trait ScheduleScope: Send {
	/// The person acting.
	fn subject(&self) -> &str;
	/// The credential a Schedule they register is admitted with later.
	fn credential(&self) -> Uuid;
	fn now(&self) -> DateTime<Utc>;
	/// Refuses unless the Tenant's policy allows `action` on the Workspace.
	async fn require_workspace(&mut self, workspace: Uuid, action: &str) -> Result<()>;
	/// Whether the person may see the Workspace.
	async fn sees_workspace(&mut self, workspace: Uuid) -> Result<bool>;
	/// Whether the Tenant's policy allows the person `schedule.administer`.
	async fn administers(&mut self) -> Result<bool>;
	/// Whether the Tenant's policy lets the person use the mode.
	async fn mode_allowed(&mut self, mode: ApprovalMode) -> Result<bool>;
	/// Refuses an Agent version of the Tenant this person could not start.
	async fn require_agent(&mut self, agent: &EntityRef) -> Result<()>;
	/// The newest version of the Agent the person could start, if newer.
	async fn newer_agent_version(&mut self, agent: &EntityRef) -> Result<Option<String>>;
	/// Whether the Thread exists, is not deleted and belongs to the Workspace.
	async fn thread_in_workspace(&mut self, workspace: Uuid, thread: Uuid) -> Result<bool>;
	/// Whether `subject` names an enabled person of the Tenant.
	async fn is_person(&mut self, subject: &str) -> Result<bool>;
	/// The Tenant's limits as stored, the Deployment's ceilings and the
	/// Tier's cap on the Tenant total, if any.
	async fn limits(&mut self) -> Result<(Limits, Ceilings, Option<u32>)>;
	async fn save_limits(&mut self, limits: &Limits) -> Result<()>;
	/// The Tenant's Schedules and how many `registrant` holds. The adapter
	/// serializes registrations of one Tenant so two cannot pass together.
	async fn counts(&mut self, registrant: &str) -> Result<(i64, i64)>;
	async fn insert(&mut self, schedule: &Schedule) -> Result<()>;
	/// Loads and locks a Schedule for change.
	async fn load_for_update(&mut self, id: Uuid) -> Result<Schedule>;
	async fn load(&mut self, id: Uuid) -> Result<Schedule>;
	/// Saves a loaded Schedule; the adapter raises its revision.
	async fn save(&mut self, schedule: &Schedule) -> Result<()>;
	async fn delete(&mut self, id: Uuid) -> Result<()>;
	async fn list(&mut self, listing: Listing) -> Result<Vec<Schedule>>;
	/// The latest ticks of a Schedule, newest first.
	async fn ticks(&mut self, id: Uuid, limit: u32) -> Result<Vec<Tick>>;
}

/// Who registers a Schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
	/// A person in the dashboard.
	Person,
	/// An Agent, for the person at the root of this Run's chain (#557 decision 6).
	Agent { run_id: Uuid },
}

/// A Schedule with what its page shows beside it.
#[derive(Debug, Clone)]
pub struct Detail {
	pub schedule: Schedule,
	pub ticks: Vec<Tick>,
	/// A newer version of its Agent the Registrant could move it to.
	pub newer_agent_version: Option<String>,
	/// What the person viewing it may do.
	pub may_edit: bool,
	pub may_administer: bool,
}

/// The most ticks a Schedule's page shows.
pub const HISTORY_LIMIT: u32 = 50;

async fn effective_limits(scope: &mut dyn ScheduleScope) -> Result<Limits> {
	let (limits, ceilings, tier) = scope.limits().await?;
	Ok(ceilings.effective(limits, tier))
}

/// Checks everything a definition names as the person who will be its
/// Registrant: the Workspace, the Agent version, the Thread and the mode.
async fn check_definition(
	scope: &mut dyn ScheduleScope,
	workspace: Uuid,
	definition: &Definition,
) -> Result<()> {
	let limits = effective_limits(scope).await?;
	definition.validate(limits.min_interval_seconds, scope.now())?;
	// Its Runs start Tasks here, so the Registrant must be able to.
	scope.require_workspace(workspace, "task.create").await?;
	scope.require_agent(&definition.agent).await?;
	if let ThreadMode::Fixed { thread_id } = definition.thread
		&& !scope.thread_in_workspace(workspace, thread_id).await?
	{
		return Err(Error::Invalid(
			"the Schedule's Thread is not a Thread of its Workspace".into(),
		));
	}
	if let Some(mode) = definition.approval_mode
		&& !scope.mode_allowed(mode).await?
	{
		return Err(Error::Invalid(
			"the Tenant's policy does not let you use that Approval Mode".into(),
		));
	}
	Ok(())
}

/// Registers a Schedule in `workspace` acting as the scope's person.
pub async fn register(
	scope: &mut dyn ScheduleScope,
	workspace: Uuid,
	definition: Definition,
	origin: Origin,
) -> Result<Schedule> {
	scope.require_workspace(workspace, "workspace.read").await?;
	scope
		.require_workspace(workspace, schedules::CREATE)
		.await?;
	check_definition(scope, workspace, &definition).await?;
	let registrant = scope.subject().to_owned();
	let (total, mine) = scope.counts(&registrant).await?;
	effective_limits(scope).await?.admit(total, mine)?;
	let now = scope.now();
	let mut schedule = Schedule {
		id: Uuid::new_v4(),
		workspace_id: workspace,
		registrant,
		credential_id: scope.credential(),
		definition,
		anchor: now,
		next_scheduled_at: None,
		halt: None,
		halted_at: None,
		successor: None,
		consecutive_failures: 0,
		created_by_run_id: match origin {
			Origin::Person => None,
			Origin::Agent { run_id } => Some(run_id),
		},
		revision: 0,
		created_at: now,
		updated_at: now,
	};
	schedule.restart(now)?;
	scope.insert(&schedule).await?;
	Ok(schedule)
}

async fn requester(scope: &mut dyn ScheduleScope) -> Result<(String, bool)> {
	let administers = scope.administers().await?;
	Ok((scope.subject().to_owned(), administers))
}

/// Loads a Schedule the person can see, or reports it missing.
async fn visible(scope: &mut dyn ScheduleScope, id: Uuid, lock: bool) -> Result<Schedule> {
	let schedule = if lock {
		scope.load_for_update(id).await?
	} else {
		scope.load(id).await?
	};
	if schedule.registrant == scope.subject()
		|| scope.sees_workspace(schedule.workspace_id).await?
		|| scope.administers().await?
	{
		Ok(schedule)
	} else {
		Err(Error::NotFound("schedule not found".into()))
	}
}

async fn permitted(
	scope: &mut dyn ScheduleScope,
	schedule: &Schedule,
	change: Change,
) -> Result<()> {
	let (subject, administers) = requester(scope).await?;
	schedules::permit(
		Requester {
			subject: &subject,
			administers,
		},
		schedule.standing(),
		change,
	)?;
	Ok(())
}

/// Changes a Schedule's settings. Only its Registrant may, and only while it
/// holds `schedule.create` there.
pub async fn update(scope: &mut dyn ScheduleScope, id: Uuid, patch: Patch) -> Result<Schedule> {
	let mut schedule = visible(scope, id, true).await?;
	permitted(scope, &schedule, Change::Edit).await?;
	if patch.is_empty() {
		return Ok(schedule);
	}
	scope
		.require_workspace(schedule.workspace_id, schedules::CREATE)
		.await?;
	let (definition, retimed) = patch.apply(&schedule.definition);
	check_definition(scope, schedule.workspace_id, &definition).await?;
	schedule.definition = definition;
	let now = scope.now();
	if retimed && schedule.halt.is_none() {
		schedule.restart(now)?;
	}
	schedule.updated_at = now;
	scope.save(&schedule).await?;
	Ok(schedule)
}

pub async fn pause(scope: &mut dyn ScheduleScope, id: Uuid) -> Result<Schedule> {
	let mut schedule = visible(scope, id, true).await?;
	permitted(scope, &schedule, Change::Pause).await?;
	if schedule.halt.is_some() {
		return Ok(schedule);
	}
	let now = scope.now();
	schedule.stop(Halt::Paused, now);
	schedule.updated_at = now;
	scope.save(&schedule).await?;
	Ok(schedule)
}

/// Resumes a Schedule as the person's own action. A named successor takes
/// it over and becomes its Registrant; nobody else resumes one whose
/// Registrant left (ADR 0134).
pub async fn resume(scope: &mut dyn ScheduleScope, id: Uuid) -> Result<Schedule> {
	let mut schedule = visible(scope, id, true).await?;
	let (subject, administers) = requester(scope).await?;
	let resumption = schedules::resumption(
		Requester {
			subject: &subject,
			administers,
		},
		schedule.standing(),
	)?;
	// Resuming is registering again: the person must still be allowed to,
	// within the limits, with what the Schedule names.
	scope
		.require_workspace(schedule.workspace_id, schedules::CREATE)
		.await?;
	check_definition(scope, schedule.workspace_id, &schedule.definition).await?;
	if resumption == Resumption::Successor {
		let (total, mine) = scope.counts(&subject).await?;
		// The Schedule is already counted in the Tenant's total.
		effective_limits(scope).await?.admit(total - 1, mine)?;
		schedule.registrant = subject;
		schedule.credential_id = scope.credential();
		schedule.created_by_run_id = None;
	}
	let now = scope.now();
	schedule.halt = None;
	schedule.halted_at = None;
	schedule.successor = None;
	schedule.consecutive_failures = 0;
	schedule.restart(now)?;
	schedule.updated_at = now;
	scope.save(&schedule).await?;
	Ok(schedule)
}

pub async fn delete(scope: &mut dyn ScheduleScope, id: Uuid) -> Result<()> {
	let schedule = visible(scope, id, true).await?;
	permitted(scope, &schedule, Change::Delete).await?;
	scope.delete(id).await
}

/// Names who may take over a Schedule whose Registrant left.
pub async fn name_successor(
	scope: &mut dyn ScheduleScope,
	id: Uuid,
	successor: &str,
) -> Result<Schedule> {
	let mut schedule = visible(scope, id, true).await?;
	permitted(scope, &schedule, Change::NameSuccessor).await?;
	if !scope.is_person(successor).await? {
		return Err(Error::Invalid(
			"a successor must be a person of the Tenant".into(),
		));
	}
	schedule.successor = Some(successor.to_owned());
	schedule.updated_at = scope.now();
	scope.save(&schedule).await?;
	Ok(schedule)
}

pub async fn list(scope: &mut dyn ScheduleScope, listing: Listing) -> Result<Vec<Schedule>> {
	match listing {
		Listing::Workspace(workspace) => {
			scope.require_workspace(workspace, "workspace.read").await?;
		}
		Listing::Mine => {}
		Listing::Tenant => {
			if !scope.administers().await? {
				return Err(Error::Forbidden);
			}
		}
	}
	scope.list(listing).await
}

pub async fn detail(scope: &mut dyn ScheduleScope, id: Uuid) -> Result<Detail> {
	let schedule = visible(scope, id, false).await?;
	let ticks = scope.ticks(id, HISTORY_LIMIT).await?;
	let newer_agent_version = scope
		.newer_agent_version(&schedule.definition.agent)
		.await?;
	let (subject, administers) = requester(scope).await?;
	let may_edit = schedules::permit(
		Requester {
			subject: &subject,
			administers,
		},
		schedule.standing(),
		Change::Edit,
	)
	.is_ok();
	Ok(Detail {
		schedule,
		ticks,
		newer_agent_version,
		may_edit,
		may_administer: administers,
	})
}

/// The Tenant's limits as set, and the ceilings they must stay within.
pub async fn limits(scope: &mut dyn ScheduleScope) -> Result<(Limits, Ceilings)> {
	let (limits, ceilings, _) = scope.limits().await?;
	Ok((limits, ceilings))
}

pub async fn set_limits(scope: &mut dyn ScheduleScope, limits: Limits) -> Result<Limits> {
	if !scope.administers().await? {
		return Err(Error::Forbidden);
	}
	let (_, ceilings, _) = scope.limits().await?;
	ceilings.permits(&limits)?;
	scope.save_limits(&limits).await?;
	Ok(limits)
}

/// What an Agent sends to the Schedule tools (#557 decision 5). The Schedule
/// is registered for the person at the root of the Run's chain, in the Run's
/// Workspace.
pub mod tool {
	use aidash_domain::{
		approval_rules::ApprovalMode,
		registry::EntityRef,
		schedules::{ModeChoice, Trigger},
	};
	use schemars::JsonSchema;
	use serde::Deserialize;
	use uuid::Uuid;

	/// Where a Schedule's Runs are started.
	#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
	#[serde(rename_all = "snake_case")]
	pub enum Thread {
		/// A new Thread on every tick.
		#[default]
		New,
		/// Always the Thread this Run works in.
		This,
	}

	#[derive(Debug, Clone, Deserialize, JsonSchema)]
	#[serde(deny_unknown_fields)]
	pub struct Create {
		/// A short name people see in the Schedule list.
		pub title: String,
		/// What the Agent is asked on every tick, written as a person would ask it.
		pub request: String,
		/// The Agent version to start; this Run's own Agent when omitted.
		#[serde(default)]
		pub agent: Option<EntityRef>,
		/// `{"kind":"cron","expression":"0 9 * * 1-5","time_zone":"Asia/Tokyo"}`
		/// (minute hour day-of-month month day-of-week) or
		/// `{"kind":"interval","every_seconds":3600}`.
		pub trigger: Trigger,
		#[serde(default)]
		pub thread: Thread,
		/// The Approval Mode of its Runs; the person's own default when omitted.
		#[serde(default)]
		pub approval_mode: Option<ApprovalMode>,
		/// How late, in seconds, a tick missed while Aidash was down may still run.
		#[serde(default)]
		pub max_lateness_seconds: Option<i64>,
	}

	#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
	#[serde(deny_unknown_fields)]
	pub struct List {}

	#[derive(Debug, Clone, Deserialize, JsonSchema)]
	#[serde(deny_unknown_fields)]
	pub struct Update {
		pub schedule_id: Uuid,
		#[serde(default)]
		pub title: Option<String>,
		#[serde(default)]
		pub request: Option<String>,
		#[serde(default)]
		pub agent: Option<EntityRef>,
		#[serde(default)]
		pub trigger: Option<Trigger>,
		#[serde(default)]
		pub thread: Option<Thread>,
		/// `default` returns to the person's own default mode.
		#[serde(default)]
		pub approval_mode: Option<ModeChoice>,
		#[serde(default)]
		pub max_lateness_seconds: Option<i64>,
	}

	/// What `schedule_delete` does.
	#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
	#[serde(rename_all = "snake_case")]
	pub enum Removal {
		#[default]
		Delete,
		/// Keep the Schedule but stop it firing until the person resumes it.
		Pause,
	}

	#[derive(Debug, Clone, Deserialize, JsonSchema)]
	#[serde(deny_unknown_fields)]
	pub struct Delete {
		pub schedule_id: Uuid,
		#[serde(default)]
		pub action: Removal,
	}
}

#[cfg(test)]
mod tests;
