//! Schedules: recurring triggers that start a Task for one Agent of the
//! Tenant with a fixed request (#557, ADR 0134). A Schedule always acts as one
//! person, its Registrant: every Run it starts has the Registrant at the root
//! of its delegation chain. There are no Tenant-owned Schedules, and a
//! Schedule whose Registrant leaves stops rather than changes hands silently.
use crate::{Error, Result, approval_rules::ApprovalMode, registry::EntityRef};
use chrono::{DateTime, Duration, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod cron;

/// Registering Schedules, and changing, pausing and deleting your own. Judged
/// on the Workspace the Schedule belongs to. Members hold it by default.
pub const CREATE: &str = "schedule.create";
/// Seeing, stopping and deleting anyone's Schedules, naming a successor for a
/// Schedule whose Registrant left, and setting the Tenant's Schedule limits.
/// Judged on the Tenant. Administrators hold it.
pub const ADMINISTER: &str = "schedule.administer";
/// Failed Runs in a row after which a Schedule pauses itself.
pub const FAILURE_LIMIT: i32 = 3;
/// The shortest gap between two fire times a Deployment allows by default.
pub const DEFAULT_MIN_INTERVAL_SECONDS: i64 = 15 * 60;
/// The longest fixed interval: one year.
pub const MAX_INTERVAL_SECONDS: i64 = 366 * 24 * 60 * 60;
/// How late a tick may still run by default, and the bounds a person may set.
pub const DEFAULT_MAX_LATENESS_SECONDS: i64 = 60 * 60;
pub const MAX_LATENESS_SECONDS: std::ops::RangeInclusive<i64> = 60..=7 * 24 * 60 * 60;
/// The most a tick is delayed past its scheduled time, so many Schedules on
/// the same minute do not start at once. It is never shown.
pub const MAX_JITTER_SECONDS: i64 = 30;
pub const TITLE_MAX: usize = 200;
pub const REQUEST_MAX: usize = 16_000;
/// Fire times checked against the minimum interval when a cron expression is
/// registered. Gaps of a cron expression repeat within this many.
const INTERVAL_CHECKS: usize = 1_000;
/// Missed fire times walked while catching up after downtime.
const CATCH_UP_LIMIT: usize = 100_000;

/// When a Schedule fires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Trigger {
	/// A five-field cron expression (minute, hour, day of month, month, day of
	/// week) read in an IANA time zone.
	Cron {
		expression: String,
		time_zone: String,
	},
	/// A fixed gap, counted from when the Schedule was registered or resumed.
	Interval { every_seconds: i64 },
}

impl Trigger {
	/// Checks the trigger and that no two fire times are closer than
	/// `min_interval_seconds`.
	pub fn validate(&self, min_interval_seconds: i64, now: DateTime<Utc>) -> Result<()> {
		match self {
			Self::Interval { every_seconds } => {
				if *every_seconds < min_interval_seconds {
					return Err(too_frequent(min_interval_seconds));
				}
				if *every_seconds > MAX_INTERVAL_SECONDS {
					return Err(Error::Invalid(
						"a Schedule's interval may be at most one year".into(),
					));
				}
				Ok(())
			}
			Self::Cron {
				expression,
				time_zone,
			} => {
				let parsed = cron::Cron::parse(expression)?;
				let zone = cron::time_zone(time_zone)?;
				if parsed.shortest_local_gap_minutes().saturating_mul(60) < min_interval_seconds {
					return Err(too_frequent(min_interval_seconds));
				}
				let mut previous = parsed.next_after(zone, now).ok_or_else(|| {
					Error::Invalid(format!("cron expression {expression:?} never fires"))
				})?;
				for _ in 0..INTERVAL_CHECKS {
					let Some(next) = parsed.next_after(zone, previous) else {
						break;
					};
					if (next - previous).num_seconds() < min_interval_seconds {
						return Err(too_frequent(min_interval_seconds));
					}
					previous = next;
				}
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

fn too_frequent(min_interval_seconds: i64) -> Error {
	Error::Invalid(format!(
		"a Schedule may fire at most once every {} minutes",
		min_interval_seconds / 60
	))
}

/// Which Thread a tick's Run is started in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ThreadMode {
	/// A new Thread on every tick.
	#[default]
	New,
	/// Always the same Thread of the Schedule's Workspace.
	Fixed { thread_id: Uuid },
}

/// Why a Schedule does not run. Only its Registrant resumes it, except that a
/// named successor resumes one whose Registrant left and so becomes its
/// Registrant (ADR 0134).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Halt {
	/// Its Registrant or an administrator paused it.
	Paused,
	/// [`FAILURE_LIMIT`] Runs in a row failed.
	Failures,
	/// The Registrant left the Tenant or lost access; it waits for a successor.
	RegistrantLeft,
	/// The pinned Agent version can no longer be started.
	AgentVersionUnusable,
	/// Its fixed Thread was deleted.
	ThreadDeleted,
	/// The Tenant was deleted. A restore leaves it stopped.
	TenantDeleted,
}

impl Halt {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Paused => "paused",
			Self::Failures => "failures",
			Self::RegistrantLeft => "registrant_left",
			Self::AgentVersionUnusable => "agent_version_unusable",
			Self::ThreadDeleted => "thread_deleted",
			Self::TenantDeleted => "tenant_deleted",
		}
	}

	pub fn parse(value: &str) -> Result<Self> {
		Ok(match value {
			"paused" => Self::Paused,
			"failures" => Self::Failures,
			"registrant_left" => Self::RegistrantLeft,
			"agent_version_unusable" => Self::AgentVersionUnusable,
			"thread_deleted" => Self::ThreadDeleted,
			"tenant_deleted" => Self::TenantDeleted,
			other => return Err(Error::Invalid(format!("unknown Schedule halt {other}"))),
		})
	}

	/// Whether the Registrant is told: every stop nobody chose, so a person
	/// who paused a Schedule hears nothing about it.
	pub fn notifies(self) -> bool {
		!matches!(self, Self::Paused | Self::TenantDeleted)
	}
}

/// What a person registers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Definition {
	pub title: String,
	/// What the Agent is asked on every tick.
	pub request: String,
	/// The Agent of the Schedule's Tenant and the version it is pinned to.
	pub agent: EntityRef,
	pub trigger: Trigger,
	#[serde(default)]
	pub thread: ThreadMode,
	/// The Approval Mode of its Runs. None follows the Registrant's own default.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub approval_mode: Option<ApprovalMode>,
	#[serde(default = "default_lateness")]
	pub max_lateness_seconds: i64,
}

fn default_lateness() -> i64 {
	DEFAULT_MAX_LATENESS_SECONDS
}

impl Definition {
	pub fn validate(&self, min_interval_seconds: i64, now: DateTime<Utc>) -> Result<()> {
		let title = self.title.trim();
		if title.is_empty()
			|| title.chars().count() > TITLE_MAX
			|| title.chars().any(char::is_control)
		{
			return Err(Error::Invalid(format!(
				"a Schedule's title needs 1 to {TITLE_MAX} characters without control characters"
			)));
		}
		if self.request.trim().is_empty() || self.request.chars().count() > REQUEST_MAX {
			return Err(Error::Invalid(format!(
				"a Schedule's request needs 1 to {REQUEST_MAX} characters"
			)));
		}
		if self.agent.id.trim().is_empty() || self.agent.version.trim().is_empty() {
			return Err(Error::Invalid(
				"a Schedule names one Agent and its version".into(),
			));
		}
		if !MAX_LATENESS_SECONDS.contains(&self.max_lateness_seconds) {
			return Err(Error::Invalid(
				"a Schedule's maximum lateness must be between one minute and seven days".into(),
			));
		}
		self.trigger.validate(min_interval_seconds, now)
	}
}

/// The hidden delay of every tick of one Schedule, from its id, so replicas
/// and restarts agree on it.
pub fn jitter_seconds(id: Uuid) -> i64 {
	(id.as_u128() % (MAX_JITTER_SECONDS as u128 + 1)) as i64
}

/// The timing of one stored Schedule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timing {
	pub trigger: Trigger,
	/// Where an interval counts from: registration or the last resume.
	pub anchor: DateTime<Utc>,
	pub max_lateness_seconds: i64,
}

/// A stored Schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Schedule {
	pub id: Uuid,
	pub workspace_id: Uuid,
	/// The person every Run of the Schedule acts as.
	pub registrant: String,
	/// The Registrant's credential its Runs are admitted with. Revoking it, as
	/// leaving the Tenant does, stops the Schedule.
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
	pub halt: Option<Halt>,
	pub halted_at: Option<DateTime<Utc>>,
	/// Who may take over a Schedule whose Registrant left.
	pub successor: Option<String>,
	pub consecutive_failures: i32,
	/// The Run whose Agent registered it, if an Agent did.
	pub created_by_run_id: Option<Uuid>,
	pub revision: i64,
	pub created_at: DateTime<Utc>,
	pub updated_at: DateTime<Utc>,
}

impl Schedule {
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
			registrant: &self.registrant,
			halt: self.halt,
			successor: self.successor.as_deref(),
		}
	}

	/// Starts counting again from `now`: on registration, on a change of its
	/// trigger, and on every resume, so a resumed Schedule never fires for
	/// times it was not running.
	pub fn restart(&mut self, now: DateTime<Utc>) -> Result<()> {
		self.anchor = now;
		self.next_scheduled_at = self.definition.trigger.next_after(now, now)?;
		Ok(())
	}

	pub fn stop(&mut self, halt: Halt, now: DateTime<Utc>) {
		self.halt = Some(halt);
		self.halted_at = Some(now);
		if halt != Halt::RegistrantLeft {
			self.successor = None;
		}
	}

	pub fn status(&self) -> &'static str {
		match self.halt {
			None => "active",
			Some(Halt::Paused | Halt::Failures) => "paused",
			Some(_) => "stopped",
		}
	}
}

/// A change to some of a Schedule's settings; absent fields stay.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Patch {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub title: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub request: Option<String>,
	/// Move to another version of the same Agent, or another Agent.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub agent: Option<EntityRef>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub trigger: Option<Trigger>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub thread: Option<ThreadMode>,
	/// `"default"` returns to the Registrant's own default mode.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub approval_mode: Option<ModeChoice>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_lateness_seconds: Option<i64>,
}

/// An Approval Mode for a Schedule's Runs, or the Registrant's default.
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
	/// Schedule counts again from now.
	pub fn apply(&self, definition: &Definition) -> (Definition, bool) {
		let mut next = definition.clone();
		if let Some(title) = &self.title {
			next.title = title.clone();
		}
		if let Some(request) = &self.request {
			next.request = request.clone();
		}
		if let Some(agent) = &self.agent {
			next.agent = agent.clone();
		}
		if let Some(trigger) = &self.trigger {
			next.trigger = trigger.clone();
		}
		if let Some(thread) = self.thread {
			next.thread = thread;
		}
		if let Some(choice) = self.approval_mode {
			next.approval_mode = choice.mode();
		}
		if let Some(lateness) = self.max_lateness_seconds {
			next.max_lateness_seconds = lateness;
		}
		let retimed = next.trigger != definition.trigger;
		(next, retimed)
	}
}

/// One tick of a Schedule, as its history shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Tick {
	pub id: Uuid,
	pub schedule_id: Uuid,
	/// The time it was scheduled for, without jitter. For "Run now", when it
	/// was clicked.
	pub scheduled_at: DateTime<Utc>,
	pub recorded_at: DateTime<Utc>,
	pub status: TickStatus,
	/// Why it was skipped or failed.
	pub reason: Option<String>,
	pub task_id: Option<Uuid>,
	pub thread_id: Option<Uuid>,
	/// The person who clicked "Run now"; None for a scheduled tick.
	pub started_by: Option<String>,
}

/// What the scheduler does with a Schedule whose next tick is due.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
	/// The tick handled: the latest scheduled time not after `now`. Earlier
	/// ticks missed while Aidash was down are folded into it, so a Schedule
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
		let mut scheduled_at = due;
		let mut next = self.trigger.next_after(self.anchor, scheduled_at)?;
		let mut walked = 0;
		while let Some(candidate) = next.filter(|candidate| *candidate <= now) {
			walked += 1;
			if walked > CATCH_UP_LIMIT {
				// Never loop without bound on a corrupt row: jump to now.
				next = self.trigger.next_after(self.anchor, now)?;
				scheduled_at = now;
				break;
			}
			scheduled_at = candidate;
			next = self.trigger.next_after(self.anchor, candidate)?;
		}
		Ok(Some(Claim {
			scheduled_at,
			late: (now - scheduled_at).num_seconds() > self.max_lateness_seconds,
			next,
		}))
	}
}

/// The Tenant's Schedule limits as its administrators set them (#557
/// decision 16), always within the Deployment's ceilings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Limits {
	/// Schedules in the whole Tenant.
	pub max_schedules: i32,
	/// Schedules one person is the Registrant of.
	pub max_per_person: i32,
	/// The shortest gap between two fire times, at least the Deployment's.
	pub min_interval_seconds: i64,
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			max_schedules: 50,
			max_per_person: 10,
			min_interval_seconds: DEFAULT_MIN_INTERVAL_SECONDS,
		}
	}
}

/// What a Deployment allows every Tenant: the most Schedules a Tenant and a
/// person may have, and the shortest interval, which a Tenant may only raise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Ceilings {
	pub max_schedules: i32,
	pub max_per_person: i32,
	pub min_interval_seconds: i64,
}

impl Default for Ceilings {
	fn default() -> Self {
		Self {
			max_schedules: 1_000,
			max_per_person: 100,
			min_interval_seconds: DEFAULT_MIN_INTERVAL_SECONDS,
		}
	}
}

impl Ceilings {
	pub fn validate(&self) -> Result<()> {
		if self.max_schedules < 1 || self.max_per_person < 1 {
			return Err(Error::Invalid(
				"the Schedule ceilings must allow at least one Schedule".into(),
			));
		}
		if !(60..=MAX_INTERVAL_SECONDS).contains(&self.min_interval_seconds) {
			return Err(Error::Invalid(
				"the minimum Schedule interval must be between one minute and one year".into(),
			));
		}
		Ok(())
	}

	/// Refuses Tenant limits beyond the ceilings.
	pub fn permits(&self, limits: &Limits) -> Result<()> {
		if limits.max_schedules < 1 || limits.max_per_person < 1 {
			return Err(Error::Invalid(
				"the Schedule limits must allow at least one Schedule".into(),
			));
		}
		if limits.max_schedules > self.max_schedules {
			return Err(Error::Invalid(format!(
				"max_schedules exceeds the Deployment's ceiling of {}",
				self.max_schedules
			)));
		}
		if limits.max_per_person > self.max_per_person {
			return Err(Error::Invalid(format!(
				"max_per_person exceeds the Deployment's ceiling of {}",
				self.max_per_person
			)));
		}
		if limits.min_interval_seconds < self.min_interval_seconds {
			return Err(Error::Invalid(format!(
				"min_interval_seconds may only raise the Deployment's minimum of {}",
				self.min_interval_seconds
			)));
		}
		if limits.min_interval_seconds > MAX_INTERVAL_SECONDS {
			return Err(Error::Invalid(
				"min_interval_seconds may be at most one year".into(),
			));
		}
		Ok(())
	}

	/// The limits that apply: stored ones clamped to the current ceilings, so
	/// lowering a ceiling takes effect without rewriting every Tenant. A
	/// Tier limit, on Cloud, lowers the Tenant total further.
	pub fn effective(&self, limits: Limits, tier: Option<u32>) -> Limits {
		let tier = tier.map_or(i32::MAX, |tier| i32::try_from(tier).unwrap_or(i32::MAX));
		Limits {
			max_schedules: limits.max_schedules.min(self.max_schedules).min(tier),
			max_per_person: limits.max_per_person.min(self.max_per_person),
			min_interval_seconds: limits.min_interval_seconds.max(self.min_interval_seconds),
		}
	}
}

impl Limits {
	/// Refuses one more Schedule for a person who already has `mine` of the
	/// Tenant's `total`.
	pub fn admit(&self, total: i64, mine: i64) -> Result<()> {
		if total >= i64::from(self.max_schedules) {
			return Err(Error::Conflict(format!(
				"SCHEDULE_LIMIT: the Tenant has its limit of {} Schedules",
				self.max_schedules
			)));
		}
		if mine >= i64::from(self.max_per_person) {
			return Err(Error::Conflict(format!(
				"SCHEDULE_LIMIT: you have your limit of {} Schedules",
				self.max_per_person
			)));
		}
		Ok(())
	}
}

/// Who asks to change a Schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requester<'a> {
	pub subject: &'a str,
	/// The Tenant's policy allows them [`ADMINISTER`].
	pub administers: bool,
}

/// Something a person may do to a Schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
	/// Change what it asks, of whom, when or where.
	Edit,
	Pause,
	Delete,
	Resume,
	/// Name who may take over a Schedule whose Registrant left.
	NameSuccessor,
}

/// The state of a Schedule a change is judged on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standing<'a> {
	pub registrant: &'a str,
	pub halt: Option<Halt>,
	pub successor: Option<&'a str>,
}

/// What a permitted resume does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resumption {
	/// The Registrant resumes their own Schedule.
	Registrant,
	/// The named successor takes the Schedule over and becomes its Registrant.
	Successor,
}

/// Whether `requester` may make `change` to a Schedule. Nobody can make a
/// Schedule run in another person's name: only its Registrant resumes or
/// edits it, and a successor resumes it only as their own (ADR 0134).
pub fn permit(requester: Requester<'_>, standing: Standing<'_>, change: Change) -> Result<()> {
	let own = requester.subject == standing.registrant;
	let allowed = match change {
		Change::Edit => own && standing.halt != Some(Halt::RegistrantLeft),
		Change::Pause | Change::Delete => own || requester.administers,
		Change::NameSuccessor => {
			requester.administers && standing.halt == Some(Halt::RegistrantLeft)
		}
		Change::Resume => return resumption(requester, standing).map(|_| ()),
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
			"only the Schedule's Registrant can change it, and not while it waits for a successor"
		}
		Change::Pause | Change::Delete => {
			"only the Schedule's Registrant or a holder of schedule.administer can do that"
		}
		Change::NameSuccessor => {
			"a successor is named by a holder of schedule.administer, for a Schedule whose Registrant left"
		}
		Change::Resume => {
			"only the Schedule's Registrant, or the successor named for it, can resume it"
		}
	}
}

/// How `requester` may resume a Schedule, if at all.
pub fn resumption(requester: Requester<'_>, standing: Standing<'_>) -> Result<Resumption> {
	match standing.halt {
		None => Err(Error::Conflict("the Schedule is already running".into())),
		Some(Halt::RegistrantLeft) if standing.successor == Some(requester.subject) => {
			Ok(Resumption::Successor)
		}
		Some(Halt::RegistrantLeft) => Err(Error::Invalid(refusal(Change::Resume).into())),
		Some(_) if requester.subject == standing.registrant => Ok(Resumption::Registrant),
		Some(_) => Err(Error::Invalid(refusal(Change::Resume).into())),
	}
}

/// What the Run calling a Schedule tool is, for the rules on Agents
/// registering Schedules (#557 decisions 6 and 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caller<'a> {
	/// The person at the root of the Run's delegation chain, if it is one.
	pub root_person: Option<&'a str>,
	/// The Run arrived through Federation.
	pub federated: bool,
	/// The Run, or a Run whose Task it descends from, was started by a Schedule.
	pub scheduled: bool,
}

/// Refuses a Schedule tool call by an Agent that may not register or change
/// Schedules, and returns the person it acts for otherwise.
pub fn agent_registrant<'a>(caller: Caller<'a>, changes: bool) -> Result<&'a str> {
	if caller.federated {
		return Err(Error::Invalid(
			"a Run that arrived through Federation cannot use Schedules".into(),
		));
	}
	if changes && caller.scheduled {
		return Err(Error::Invalid(
			"a Run started by a Schedule, and anything it delegates to, cannot register or change Schedules".into(),
		));
	}
	caller.root_person.ok_or_else(|| {
		Error::Invalid(
			"Schedules act as a person, and this Run's delegation chain does not start at one"
				.into(),
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

	/// How a started tick ends when its Task does. None while it runs.
	pub fn of_task(status: crate::TaskStatus) -> Option<Self> {
		use crate::TaskStatus as Task;
		match status {
			Task::Completed => Some(Self::Completed),
			Task::Failed | Task::Blocked => Some(Self::Failed),
			Task::Cancelled | Task::Abandoned => Some(Self::Cancelled),
			Task::Open | Task::Claimed | Task::Running => None,
		}
	}
}

/// Why a tick was skipped.
pub mod skip {
	/// The previous tick's Run had not finished.
	pub const OVERLAP: &str = "overlap";
	/// Aidash was down past the Schedule's maximum lateness.
	pub const LATE: &str = "late";
	/// The usage gate refused the Run; `limit: <code>` carries its code.
	pub const LIMIT: &str = "limit";
	/// It came sooner than the minimum interval after the previous tick, as a
	/// daylight-saving change can make it.
	pub const INTERVAL: &str = "interval";
}

/// Whether a tick at `scheduled_at` comes sooner than the minimum interval
/// after the previous scheduled tick, which then refuses it.
pub fn too_soon(
	previous: Option<DateTime<Utc>>,
	scheduled_at: DateTime<Utc>,
	min_interval_seconds: i64,
) -> bool {
	previous.is_some_and(|previous| (scheduled_at - previous).num_seconds() < min_interval_seconds)
}

/// Consecutive failures after one more settled tick, and whether the
/// Schedule now pauses itself (#557 decision 18). Skipped ticks never reach
/// here; a cancelled Run leaves the count as it was.
pub fn after_settled(failures: i32, status: TickStatus) -> (i32, bool) {
	match status {
		TickStatus::Completed => (0, false),
		TickStatus::Failed => {
			let failures = failures.saturating_add(1);
			(failures, failures >= FAILURE_LIMIT)
		}
		TickStatus::Started | TickStatus::Cancelled | TickStatus::Skipped => (failures, false),
	}
}

#[cfg(test)]
mod tests;
