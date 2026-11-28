//! Duties: standing responsibilities a Node carries out for a person (#1011,
//! ADR 0142). A Duty has a mission, a procedure, Duty Triggers and its own
//! Thread, and one Agent carries it out. It absorbs the Schedules of #557: a
//! time trigger is one kind of Duty Trigger.
//!
//! A Duty has no authority of its own. It always acts as one person, its
//! Requester: every Run it starts has the Requester at the root of its
//! delegation chain (ADR 0134). A Duty whose Requester leaves stops rather
//! than changes hands silently.
use crate::{Error, Result, approval_rules::ApprovalMode, registry::EntityRef};
use chrono::{DateTime, Duration, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod cron;
pub mod events;
pub mod scripts;
pub mod templates;

/// Registering Duties, and changing, pausing and deleting your own. Judged
/// on the Thread the Duty belongs to. Members hold it by default.
pub const CREATE: &str = "duty.create";
/// Seeing, stopping and deleting anyone's Duties and naming a successor for
/// a Duty whose Requester left. Judged on the Tenant. Administrators hold it.
pub const ADMINISTER: &str = "duty.administer";
/// Failed Runs in a row after which a Duty pauses itself, unless its
/// Requester chooses another number or 0, which never pauses it.
pub const DEFAULT_PAUSE_AFTER_FAILURES: i32 = 3;
pub const MAX_PAUSE_AFTER_FAILURES: i32 = 1_000;
/// The longest fixed interval: one year.
pub const MAX_INTERVAL_SECONDS: i64 = 366 * 24 * 60 * 60;
/// How late a tick may still run by default, and the bounds a person may set.
pub const DEFAULT_MAX_LATENESS_SECONDS: i64 = 60 * 60;
pub const MAX_LATENESS_SECONDS: std::ops::RangeInclusive<i64> = 60..=7 * 24 * 60 * 60;
/// The most a tick is delayed past its scheduled time, so many Duties on
/// the same minute do not start at once. It is never shown.
pub const MAX_JITTER_SECONDS: i64 = 30;
pub const TITLE_MAX: usize = 200;
pub const MISSION_MAX: usize = 16_000;
pub const PROCEDURE_MAX: usize = 16_000;
/// Missed fire times walked while catching up after downtime.
const CATCH_UP_LIMIT: usize = 100_000;

/// When a Duty fires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Trigger {
	/// A five-field cron expression (minute, hour, day of month, month, day of
	/// week) read in an IANA time zone.
	Cron {
		expression: String,
		time_zone: String,
	},
	/// A fixed gap, counted from when the Duty was registered or resumed.
	Interval { every_seconds: i64 },
}

impl Trigger {
	/// Checks the trigger. There is no minimum gap between fire times
	/// (#1012): a Budget, not a count, bounds what a Duty spends.
	pub fn validate(&self, now: DateTime<Utc>) -> Result<()> {
		match self {
			Self::Interval { every_seconds } => {
				if !(1..=MAX_INTERVAL_SECONDS).contains(every_seconds) {
					return Err(Error::Invalid(
						"a Duty's interval must be between one second and one year".into(),
					));
				}
				Ok(())
			}
			Self::Cron {
				expression,
				time_zone,
			} => {
				let parsed = cron::Cron::parse(expression)?;
				parsed
					.next_after(cron::time_zone(time_zone)?, now)
					.ok_or_else(|| {
						Error::Invalid(format!("cron expression {expression:?} never fires"))
					})?;
				Ok(())
			}
		}
	}

	/// The first scheduled time strictly after `after`. An interval counts
	/// from `anchor`.
	pub fn next_after(
		&self,
		anchor: DateTime<Utc>,
		after: DateTime<Utc>,
	) -> Result<Option<DateTime<Utc>>> {
		match self {
			Self::Interval { every_seconds } => {
				if *every_seconds <= 0 {
					return Err(Error::Invalid("an interval must be positive".into()));
				}
				if after < anchor {
					return Ok(Some(anchor));
				}
				let elapsed = (after - anchor).num_seconds();
				let steps = elapsed / every_seconds + 1;
				Ok(Some(anchor + Duration::seconds(steps * every_seconds)))
			}
			Self::Cron {
				expression,
				time_zone,
			} => Ok(cron::Cron::parse(expression)?.next_after(cron::time_zone(time_zone)?, after)),
		}
	}
}

/// Why a Duty does not run. Only its Requester resumes it; a named
/// successor first takes over one whose Requester left (ADR 0145).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Halt {
	/// Its Requester or an administrator paused it.
	Paused,
	/// As many Runs in a row failed as its Requester allows.
	Failures,
	/// The Requester left the Tenant or lost access; it waits for a successor.
	RequesterLeft,
	/// The pinned Agent version can no longer be started.
	AgentVersionUnusable,
	/// Its Thread was deleted.
	ThreadDeleted,
	/// The Tenant was deleted. A restore leaves it stopped.
	TenantDeleted,
}

impl Halt {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Paused => "paused",
			Self::Failures => "failures",
			Self::RequesterLeft => "requester_left",
			Self::AgentVersionUnusable => "agent_version_unusable",
			Self::ThreadDeleted => "thread_deleted",
			Self::TenantDeleted => "tenant_deleted",
		}
	}

	pub fn parse(value: &str) -> Result<Self> {
		Ok(match value {
			"paused" => Self::Paused,
			"failures" => Self::Failures,
			"requester_left" => Self::RequesterLeft,
			"agent_version_unusable" => Self::AgentVersionUnusable,
			"thread_deleted" => Self::ThreadDeleted,
			"tenant_deleted" => Self::TenantDeleted,
			other => return Err(Error::Invalid(format!("unknown Duty halt {other}"))),
		})
	}

	/// Whether the Requester is told: every stop nobody chose, so a person
	/// who paused a Duty hears nothing about it.
	pub fn notifies(self) -> bool {
		!matches!(self, Self::Paused | Self::TenantDeleted)
	}
}

/// What a person, or an Agent for its root person, sets up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Definition {
	pub title: String,
	/// What the Duty is for; every Task it starts is asked this.
	pub mission: String,
	/// How the Agent carries the mission out: the checklist it follows on
	/// every Task (ranked after the Agent's own instructions, ADR 0142).
	#[serde(default)]
	pub procedure: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub goal: Option<String>,
	/// The Agent of the Duty's Tenant and the version it is pinned to.
	pub agent: EntityRef,
	/// When the Duty wakes by the clock, if it does.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub trigger: Option<Trigger>,
	/// The Approval Mode of its Runs. None follows the Requester's own default.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub approval_mode: Option<ApprovalMode>,
	#[serde(default = "default_lateness")]
	pub max_lateness_seconds: i64,
	/// Failed Runs in a row after which the Duty pauses itself; 0 never
	/// pauses it.
	#[serde(default = "default_pause_after_failures")]
	pub pause_after_failures: i32,
	/// The script that decides what each event no Task booked does, instead of
	/// starting one Task per event (#1011 D3).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub router: Option<scripts::Script>,
	/// The script that polls an outside service on its own interval and
	/// reports events.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub watcher: Option<scripts::Watcher>,
}

fn default_lateness() -> i64 {
	DEFAULT_MAX_LATENESS_SECONDS
}

fn default_pause_after_failures() -> i32 {
	DEFAULT_PAUSE_AFTER_FAILURES
}

impl Definition {
	pub fn validate(&self, now: DateTime<Utc>) -> Result<()> {
		crate::goal::validate_goal(self.goal.as_deref())?;
		let title = self.title.trim();
		if title.is_empty()
			|| title.chars().count() > TITLE_MAX
			|| title.chars().any(char::is_control)
		{
			return Err(Error::Invalid(format!(
				"a Duty's title needs 1 to {TITLE_MAX} characters without control characters"
			)));
		}
		if self.mission.trim().is_empty() || self.mission.chars().count() > MISSION_MAX {
			return Err(Error::Invalid(format!(
				"a Duty's mission needs 1 to {MISSION_MAX} characters"
			)));
		}
		if self.procedure.chars().count() > PROCEDURE_MAX || self.procedure.contains('\0') {
			return Err(Error::Invalid(format!(
				"a Duty's procedure may have at most {PROCEDURE_MAX} characters and no NUL"
			)));
		}
		if !(0..=MAX_PAUSE_AFTER_FAILURES).contains(&self.pause_after_failures) {
			return Err(Error::Invalid(format!(
				"pause_after_failures must be from 0 (never) to {MAX_PAUSE_AFTER_FAILURES}"
			)));
		}
		if self.agent.id.trim().is_empty() || self.agent.version.trim().is_empty() {
			return Err(Error::Invalid(
				"a Duty names one Agent and its version".into(),
			));
		}
		if !MAX_LATENESS_SECONDS.contains(&self.max_lateness_seconds) {
			return Err(Error::Invalid(
				"a Duty's maximum lateness must be between one minute and seven days".into(),
			));
		}
		if let Some(router) = &self.router {
			router.validate()?;
		}
		if let Some(watcher) = &self.watcher {
			watcher.validate()?;
		}
		match &self.trigger {
			Some(trigger) => trigger.validate(now),
			None => Ok(()),
		}
	}
}

/// The hidden delay of every tick of one Duty, from its id, so replicas
/// and restarts agree on it.
pub fn jitter_seconds(id: Uuid) -> i64 {
	(id.as_u128() % (MAX_JITTER_SECONDS as u128 + 1)) as i64
}

/// The timing of one stored Duty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timing {
	pub trigger: Option<Trigger>,
	/// Where an interval counts from: registration or the last resume.
	pub anchor: DateTime<Utc>,
	pub max_lateness_seconds: i64,
}

/// A stored Duty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Duty {
	pub id: Uuid,
	/// The Duty's own Thread, opened when it is registered: its Tasks
	/// belong there, report there, and its Requester talks to it there
	/// (#1008, ADR 0140).
	pub thread_id: Uuid,
	/// The Thread it was registered from: its Agents list and change the
	/// Duty there too. None for a Duty from before #1008, whose own Thread
	/// is the one it was registered in.
	pub origin_thread_id: Option<Uuid>,
	/// The person every Run of the Duty acts as.
	pub requester: String,
	/// The Requester's credential its Runs are admitted with. Revoking it, as
	/// leaving the Tenant does, stops the Duty.
	#[serde(skip)]
	#[schemars(skip)]
	pub credential_id: Uuid,
	#[serde(flatten)]
	pub definition: Definition,
	/// Where an interval counts from: registration or the last resume.
	pub anchor: DateTime<Utc>,
	/// The next scheduled time, shown without its jitter. None once the
	/// trigger never fires again.
	pub next_scheduled_at: Option<DateTime<Utc>>,
	/// When its Watcher runs next, if it has one.
	pub watcher_next_at: Option<DateTime<Utc>>,
	pub halt: Option<Halt>,
	pub halted_at: Option<DateTime<Utc>>,
	/// Why it stopped, in words, when the halt alone does not say it all.
	pub halt_reason: Option<String>,
	/// Who may take over a Duty whose Requester left.
	pub successor: Option<String>,
	pub consecutive_failures: i32,
	/// The Run whose Agent registered it, if an Agent did.
	pub created_by_run_id: Option<Uuid>,
	/// The tool call that registered it, so a retried call returns it.
	#[serde(skip)]
	#[schemars(skip)]
	pub creation_key: Option<String>,
	/// The Marketplace template version it was installed from, if it was
	/// (#1011 D4), so a later version can be offered as an update.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub template: Option<templates::Source>,
	pub revision: i64,
	pub created_at: DateTime<Utc>,
	pub updated_at: DateTime<Utc>,
}

impl Duty {
	pub fn timing(&self) -> Timing {
		Timing {
			trigger: self.definition.trigger.clone(),
			anchor: self.anchor,
			max_lateness_seconds: self.definition.max_lateness_seconds,
		}
	}

	/// When the scheduler may claim the next tick: its time plus the jitter.
	pub fn next_fire_at(&self) -> Option<DateTime<Utc>> {
		self.next_scheduled_at
			.map(|at| at + Duration::seconds(jitter_seconds(self.id)))
	}

	pub fn standing(&self) -> Standing<'_> {
		Standing {
			requester: &self.requester,
			halt: self.halt,
			successor: self.successor.as_deref(),
		}
	}

	/// Starts counting again from `now`: on registration, on a change of its
	/// trigger, and on every resume, so a resumed Duty never fires for
	/// times it was not running.
	pub fn restart(&mut self, now: DateTime<Utc>) -> Result<()> {
		self.anchor = now;
		self.next_scheduled_at = match &self.definition.trigger {
			Some(trigger) => trigger.next_after(now, now)?,
			None => None,
		};
		self.rewatch(now);
		Ok(())
	}

	/// Runs its Watcher, if it has one, at the next scheduler pass: on
	/// registration, on every resume and whenever the Watcher changes.
	pub fn rewatch(&mut self, now: DateTime<Utc>) {
		self.watcher_next_at = self.definition.watcher.as_ref().map(|_| now);
	}

	pub fn stop(&mut self, halt: Halt, now: DateTime<Utc>) {
		self.stop_because(halt, None, now);
	}

	/// [`Self::stop`], saying why in words.
	pub fn stop_because(&mut self, halt: Halt, reason: Option<String>, now: DateTime<Utc>) {
		self.halt = Some(halt);
		self.halted_at = Some(now);
		self.halt_reason = reason;
		if halt != Halt::RequesterLeft {
			self.successor = None;
		}
	}

	/// Makes the named successor its Requester, as their own action. It stays
	/// stopped, as paused, until they resume it (ADR 0145).
	pub fn take_over(&mut self, successor: &str, now: DateTime<Utc>) {
		self.requester = successor.to_owned();
		self.created_by_run_id = None;
		self.stop(Halt::Paused, now);
	}

	/// Clears a stop, on resume.
	pub fn clear_halt(&mut self) {
		self.halt = None;
		self.halted_at = None;
		self.halt_reason = None;
		self.successor = None;
	}

	pub fn status(&self) -> &'static str {
		match self.halt {
			None => "active",
			Some(Halt::Paused | Halt::Failures) => "paused",
			Some(_) => "stopped",
		}
	}
}

/// A change to some of a Duty's settings; absent fields stay.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Patch {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub title: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub mission: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub procedure: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub goal: Option<String>,
	/// Move to another version of the same Agent, or another Agent.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub agent: Option<EntityRef>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub trigger: Option<TriggerChoice>,
	/// `"default"` returns to the Requester's own default mode.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub approval_mode: Option<ModeChoice>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_lateness_seconds: Option<i64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub pause_after_failures: Option<i32>,
	/// A new Router, or `{"kind": "none"}` to remove it (#1011 D3).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub router: Option<scripts::ScriptChoice>,
	/// A new Watcher, or `{"kind": "none"}` to remove it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub watcher: Option<scripts::WatcherChoice>,
}

/// A new time trigger, or none: the Duty then wakes only by its other Duty
/// Triggers and "Run now".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum TriggerChoice {
	Set(Trigger),
	Remove(NoTrigger),
}

// It also removes a Router or a Watcher (#1011). Its doc comment is part of
// the `duty_update` tool declaration, so it stays as it was.
/// `{"kind": "none"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NoTrigger {
	None,
}

/// An Approval Mode for a Duty's Runs, or the Requester's default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModeChoice {
	Default,
	Manual,
	Auto,
	DontAsk,
	Bypass,
}

impl ModeChoice {
	pub fn mode(self) -> Option<ApprovalMode> {
		match self {
			Self::Default => None,
			Self::Manual => Some(ApprovalMode::Manual),
			Self::Auto => Some(ApprovalMode::Auto),
			Self::DontAsk => Some(ApprovalMode::DontAsk),
			Self::Bypass => Some(ApprovalMode::Bypass),
		}
	}
}

impl Patch {
	pub fn is_empty(&self) -> bool {
		*self == Self::default()
	}

	/// The definition after this change. Whether the trigger changed, so the
	/// Duty counts again from now.
	pub fn apply(&self, definition: &Definition) -> (Definition, bool) {
		let mut next = definition.clone();
		if let Some(title) = &self.title {
			next.title = title.clone();
		}
		if let Some(mission) = &self.mission {
			next.mission = mission.clone();
		}
		if let Some(procedure) = &self.procedure {
			next.procedure = procedure.clone();
		}
		if let Some(goal) = &self.goal {
			next.goal = (!goal.trim().is_empty()).then(|| goal.clone());
		}
		if let Some(agent) = &self.agent {
			next.agent = agent.clone();
		}
		match &self.trigger {
			Some(TriggerChoice::Set(trigger)) => next.trigger = Some(trigger.clone()),
			Some(TriggerChoice::Remove(_)) => next.trigger = None,
			None => {}
		}
		if let Some(count) = self.pause_after_failures {
			next.pause_after_failures = count;
		}
		if let Some(choice) = self.approval_mode {
			next.approval_mode = choice.mode();
		}
		if let Some(lateness) = self.max_lateness_seconds {
			next.max_lateness_seconds = lateness;
		}
		match &self.router {
			Some(scripts::ScriptChoice::Set(script)) => next.router = Some(script.clone()),
			Some(scripts::ScriptChoice::Remove(_)) => next.router = None,
			None => {}
		}
		match &self.watcher {
			Some(scripts::WatcherChoice::Set(watcher)) => next.watcher = Some(watcher.clone()),
			Some(scripts::WatcherChoice::Remove(_)) => next.watcher = None,
			None => {}
		}
		let retimed = next.trigger != definition.trigger;
		(next, retimed)
	}
}

/// One tick of a Duty, as its history shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Tick {
	pub id: Uuid,
	pub duty_id: Uuid,
	/// The time it was scheduled for, without jitter. For "Run now", when it
	/// was clicked.
	pub scheduled_at: DateTime<Utc>,
	pub recorded_at: DateTime<Utc>,
	pub status: TickStatus,
	/// Why it was skipped or failed.
	pub reason: Option<String>,
	pub task_id: Option<Uuid>,
	/// The person who clicked "Run now"; None for a scheduled tick.
	pub started_by: Option<String>,
}

/// What the scheduler does with a Duty whose next tick is due.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
	/// The tick handled: the latest scheduled time not after `now`. Earlier
	/// ticks missed while Aidash was down are folded into it, so a Duty
	/// runs once, late, rather than once per missed tick.
	pub scheduled_at: DateTime<Utc>,
	/// Too late to run: it is recorded as skipped.
	pub late: bool,
	/// The scheduled time after it, if the trigger fires again.
	pub next: Option<DateTime<Utc>>,
}

impl Timing {
	/// The tick to handle at `now`, given the stored next scheduled time.
	/// None while that time is still in the future.
	pub fn claim(&self, due: DateTime<Utc>, now: DateTime<Utc>) -> Result<Option<Claim>> {
		if due > now {
			return Ok(None);
		}
		let Some(trigger) = &self.trigger else {
			return Ok(None);
		};
		let mut scheduled_at = due;
		let mut next = trigger.next_after(self.anchor, scheduled_at)?;
		let mut walked = 0;
		while let Some(candidate) = next.filter(|candidate| *candidate <= now) {
			walked += 1;
			if walked > CATCH_UP_LIMIT {
				// Never loop without bound on a corrupt row: jump to now.
				next = trigger.next_after(self.anchor, now)?;
				scheduled_at = now;
				break;
			}
			scheduled_at = candidate;
			next = trigger.next_after(self.anchor, candidate)?;
		}
		Ok(Some(Claim {
			scheduled_at,
			late: (now - scheduled_at).num_seconds() > self.max_lateness_seconds,
			next,
		}))
	}
}

/// Refuses one more Duty when a Cloud Tier caps the Tenant's total. There is
/// no other count limit (#1012): a Budget bounds what Duties spend.
pub fn admit_within_tier(total: i64, tier: Option<u32>) -> Result<()> {
	match tier {
		Some(cap) if total >= i64::from(cap) => Err(Error::Conflict(format!(
			"DUTY_LIMIT: the Tenant's plan allows {cap} Duties"
		))),
		_ => Ok(()),
	}
}

/// Who asks to change a Duty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Actor<'a> {
	pub subject: &'a str,
	/// The Tenant's policy allows them [`ADMINISTER`].
	pub administers: bool,
}

/// Something a person may do to a Duty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
	/// Change what it asks, of whom, when or where.
	Edit,
	Pause,
	Delete,
	Resume,
	/// Name who may take over a Duty whose Requester left.
	NameSuccessor,
	/// Become the Requester of a Duty whose Requester left, as its named
	/// successor (ADR 0145).
	TakeOver,
}

/// The state of a Duty a change is judged on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standing<'a> {
	pub requester: &'a str,
	pub halt: Option<Halt>,
	pub successor: Option<&'a str>,
}

/// Whether `actor` may make `change` to a Duty. Nobody can make a
/// Duty run in another person's name: only its Requester resumes or
/// edits it, and a successor takes it over first, as their own action
/// (ADR 0134, ADR 0145).
pub fn permit(actor: Actor<'_>, standing: Standing<'_>, change: Change) -> Result<()> {
	let own = actor.subject == standing.requester;
	let waiting = standing.halt == Some(Halt::RequesterLeft);
	let allowed = match change {
		Change::Edit => own && !waiting,
		Change::Pause | Change::Delete => own || actor.administers,
		Change::NameSuccessor => actor.administers && waiting,
		Change::TakeOver => waiting && standing.successor == Some(actor.subject),
		Change::Resume => {
			if standing.halt.is_none() {
				return Err(Error::Conflict("the Duty is already running".into()));
			}
			own && !waiting
		}
	};
	if allowed {
		Ok(())
	} else {
		Err(Error::Invalid(refusal(change).into()))
	}
}

fn refusal(change: Change) -> &'static str {
	match change {
		Change::Edit => {
			"only the Duty's Requester can change it, and not while it waits for a successor"
		}
		Change::Pause | Change::Delete => {
			"only the Duty's Requester or a holder of duty.administer can do that"
		}
		Change::NameSuccessor => {
			"a successor is named by a holder of duty.administer, for a Duty whose Requester left"
		}
		Change::Resume => {
			"only the Duty's Requester can resume it; a successor takes it over first"
		}
		Change::TakeOver => {
			"only the successor named for a Duty whose Requester left can take it over"
		}
	}
}

/// Who deletes a Duty, for the Budgets that go with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deleter {
	/// A person in the dashboard or the API.
	Person,
	/// An Agent's `duty_delete`, for the person at the root of its Run.
	Agent,
}

/// What deleting a Duty asks about the Budgets that target it, which go with
/// it (#1012 decision 25).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetRemoval {
	/// No Budget targets it, or the person deleting it set every one.
	Free,
	/// Someone else set one, so only `budget.manage` removes it.
	NeedsBudgetManage,
}

/// Judges deleting a Duty that `setters` (who last set each Budget targeting
/// it) limit. An Agent never raises or removes a Budget, so it may only pause
/// a Duty a Budget targets; a person may not lift a limit someone else set on
/// them by deleting the Duty and registering it again.
pub fn budget_removal(deleter: Deleter, actor: &str, setters: &[String]) -> Result<BudgetRemoval> {
	if setters.is_empty() {
		return Ok(BudgetRemoval::Free);
	}
	match deleter {
		Deleter::Agent => Err(Error::Invalid(
			"a Budget targets this Duty and an Agent never removes a Budget: pause the Duty, or ask a person to delete it".into(),
		)),
		Deleter::Person if setters.iter().all(|setter| setter == actor) => Ok(BudgetRemoval::Free),
		Deleter::Person => Ok(BudgetRemoval::NeedsBudgetManage),
	}
}

/// Why a person without `budget.manage` cannot delete a Duty whose Budget
/// someone else set.
pub const BUDGET_SET_BY_SOMEONE_ELSE: &str = "a Budget someone else set targets this Duty, and deleting the Duty removes it: that needs budget.manage";

/// What the Run calling a Duty tool is, for the rules on Agents setting up
/// Duties. A Run a Duty started may set up Duties too (#1012): only the
/// root person's authority and Budget bound what a chain creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caller<'a> {
	/// The person at the root of the Run's delegation chain, if it is one.
	pub root_person: Option<&'a str>,
	/// The Run arrived through Federation. A Duty never runs on another
	/// Tenant's Node, so such a Run sets up none here (#1013).
	pub federated: bool,
}

/// Refuses a Duty tool call by an Agent that may not set up or change
/// Duties, and returns the person it acts for otherwise.
pub fn agent_requester<'a>(caller: Caller<'a>) -> Result<&'a str> {
	if caller.federated {
		return Err(Error::Invalid(
			"a Run that arrived through Federation cannot use Duties".into(),
		));
	}
	caller.root_person.ok_or_else(|| {
		Error::Invalid(
			"Duties act as a person, and this Run's delegation chain does not start at one".into(),
		)
	})
}

/// What became of one tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TickStatus {
	/// Its Run is under way.
	Started,
	Completed,
	/// Its Run failed, or it could not be started.
	Failed,
	/// Its Run was cancelled or abandoned; it counts neither way.
	Cancelled,
	/// It did not start; the reason says why.
	Skipped,
}

impl TickStatus {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Started => "started",
			Self::Completed => "completed",
			Self::Failed => "failed",
			Self::Cancelled => "cancelled",
			Self::Skipped => "skipped",
		}
	}

	pub fn parse(value: &str) -> Result<Self> {
		Ok(match value {
			"started" => Self::Started,
			"completed" => Self::Completed,
			"failed" => Self::Failed,
			"cancelled" => Self::Cancelled,
			"skipped" => Self::Skipped,
			other => return Err(Error::Invalid(format!("unknown tick status {other}"))),
		})
	}

	/// How a started tick ends when its Task does. None while it runs, and
	/// while it is blocked, since a blocked Task may run again.
	pub fn of_task(status: crate::TaskStatus) -> Option<Self> {
		use crate::TaskStatus as Task;
		match status {
			Task::Completed => Some(Self::Completed),
			Task::Failed => Some(Self::Failed),
			Task::Cancelled | Task::Abandoned => Some(Self::Cancelled),
			Task::Open | Task::Claimed | Task::Running | Task::Blocked => None,
		}
	}
}

/// Why a tick was skipped.
pub mod skip {
	/// The previous tick's Run had not finished.
	pub const OVERLAP: &str = "overlap";
	/// Aidash was down past the Duty's maximum lateness.
	pub const LATE: &str = "late";
	/// The usage gate refused the Run; `limit: <code>` carries its code.
	pub const LIMIT: &str = "limit";
	/// A Budget covering the Duty is at its Hard Limit; `budget: <id>` names
	/// it (#1012).
	pub const BUDGET: &str = "budget";
}

/// Consecutive failures after one more settled tick, and whether the Duty
/// now pauses itself after `pause_after` failures in a row; 0 never pauses
/// it. Skipped ticks never reach here; a cancelled Run leaves the count as
/// it was.
pub fn after_settled(failures: i32, status: TickStatus, pause_after: i32) -> (i32, bool) {
	match status {
		TickStatus::Completed => (0, false),
		TickStatus::Failed => {
			let failures = failures.saturating_add(1);
			(failures, pause_after > 0 && failures >= pause_after)
		}
		TickStatus::Started | TickStatus::Cancelled | TickStatus::Skipped => (failures, false),
	}
}

#[cfg(test)]
mod tests;
