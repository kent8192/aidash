//! Duty use cases (#1011, ADR 0142; from the Schedules of #557, ADR 0134).
//! The scope owns one transaction and acts for one person: a person managing
//! Duties in the dashboard, or the person at the root of an Agent's Run when
//! an Agent calls a Duty tool. A Duty set up here acts as that person, its
//! Requester.
use crate::{Error, Result};
use aidash_domain::{
	approval_rules::ApprovalMode,
	duties::{
		self, Actor, BudgetRemoval, Change, Definition, Deleter, Duty, Halt, Patch, Resumption,
		Tick,
		templates::{Source, Template},
	},
	registry::EntityRef,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Which Duties a list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listing {
	/// Every Duty of one Thread the person can see: the Duty whose own
	/// Thread it is and those registered from it.
	Thread(Uuid),
	/// The person's own Duties, in every Thread, and those waiting for
	/// them to take over as the named successor.
	Mine,
	/// Every Duty of the Tenant, for holders of `duty.administer`.
	Tenant,
}

/// Persistence and policy for one person's Duty transaction.
#[async_trait]
pub trait DutyScope: Send {
	/// The person acting.
	fn subject(&self) -> &str;
	/// The credential the person's Duties are admitted with: their
	/// Mapping's own long-lived credential, whichever credential they act
	/// with now, so a Duty outlives an API token and stops only when the
	/// Mapping does (ADR 0134). A subject without a Mapping keeps the
	/// credential it acts with.
	async fn requester_credential(&mut self) -> Result<Uuid>;
	/// The Duty an Agent's tool call with this key already registered.
	async fn created_by(&mut self, run: Uuid, key: &str) -> Result<Option<Duty>>;
	fn now(&self) -> DateTime<Utc>;
	/// Refuses unless the Tenant's policy allows `action` on the Thread.
	async fn require_thread(&mut self, thread: Uuid, action: &str) -> Result<()>;
	/// Whether the person may see the Thread.
	async fn sees_thread(&mut self, thread: Uuid) -> Result<bool>;
	/// Whether the Tenant's policy allows the person `duty.administer`.
	async fn administers(&mut self) -> Result<bool>;
	/// Whether the Tenant's policy lets the person use the mode.
	async fn mode_allowed(&mut self, mode: ApprovalMode) -> Result<bool>;
	/// Refuses an Agent version of the Tenant this person could not start.
	async fn require_agent(&mut self, agent: &EntityRef) -> Result<()>;
	/// The newest version of the Agent the person could start, if newer.
	async fn newer_agent_version(&mut self, agent: &EntityRef) -> Result<Option<String>>;
	/// Whether `subject` names an enabled person of the Tenant.
	async fn is_person(&mut self, subject: &str) -> Result<bool>;
	/// The Cloud Tier's cap on the Tenant's Duties, if any. There is no
	/// other count limit (#1012).
	async fn tier_cap(&mut self) -> Result<Option<u32>>;
	/// The Tenant's Duties. The adapter serializes registrations of one
	/// Tenant so two cannot pass a Tier cap together.
	async fn count(&mut self) -> Result<i64>;
	/// Opens the Duty's own Thread in the Node and Scratch Space of
	/// `from`, created by the Requester, with the Duty's title and its
	/// mission as the first message (#1008, ADR 0140).
	async fn open_thread(&mut self, from: Uuid, definition: &Definition) -> Result<Uuid>;
	async fn insert(&mut self, duty: &Duty) -> Result<()>;
	/// Loads and locks a Duty for change.
	async fn load_for_update(&mut self, id: Uuid) -> Result<Duty>;
	async fn load(&mut self, id: Uuid) -> Result<Duty>;
	/// Saves a loaded Duty; the adapter raises its revision.
	async fn save(&mut self, duty: &Duty) -> Result<()>;
	async fn delete(&mut self, id: Uuid) -> Result<()>;
	/// Hands a Duty over to its successor in this transaction, before the
	/// successor's rights are checked: the Duty's own Thread, when it opened
	/// one, becomes theirs, and the bookings Runs made for its triggers are
	/// dropped, so no event of the Duty wakes a Run acting for the departed
	/// Requester.
	async fn hand_over(&mut self, duty: &Duty, successor: &str) -> Result<()>;
	/// Who last set each Budget that targets the Duty (#1012).
	async fn budget_setters(&mut self, duty: Uuid) -> Result<Vec<String>>;
	/// Whether the Tenant's policy allows the person `budget.manage`.
	async fn manages_budgets(&mut self) -> Result<bool>;
	async fn list(&mut self, listing: Listing) -> Result<Vec<Duty>>;
	/// The latest ticks of a Duty, newest first.
	async fn ticks(&mut self, id: Uuid, limit: u32) -> Result<Vec<Tick>>;
}

/// Who registers a Duty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
	/// A person in the dashboard.
	Person,
	/// An Agent, for the person at the root of this Run's chain (#557 decision
	/// 6), in the tool call with this invocation key; a retried call returns
	/// the Duty the first one registered.
	Agent { run_id: Uuid, key: String },
}

/// A Duty with what its page shows beside it.
#[derive(Debug, Clone)]
pub struct Detail {
	pub duty: Duty,
	pub ticks: Vec<Tick>,
	/// A newer version of its Agent the Requester could move it to.
	pub newer_agent_version: Option<String>,
	/// What the person viewing it may do.
	pub may_edit: bool,
	pub may_administer: bool,
}

/// The most ticks a Duty's page shows.
pub const HISTORY_LIMIT: u32 = 50;

/// Checks everything a definition names as the person who will be its
/// Requester: the Thread, the Agent version and the mode.
async fn check_definition(
	scope: &mut dyn DutyScope,
	thread: Uuid,
	definition: &Definition,
) -> Result<()> {
	definition.validate(scope.now())?;
	// Its Runs start Tasks here, so the Requester must be able to.
	scope.require_thread(thread, "task.create").await?;
	scope.require_agent(&definition.agent).await?;
	if let Some(mode) = definition.approval_mode
		&& !scope.mode_allowed(mode).await?
	{
		return Err(Error::Invalid(
			"the Tenant's policy does not let you use that Approval Mode".into(),
		));
	}
	Ok(())
}

/// Registers a Duty from `thread` acting as the scope's person. The Duty
/// opens its own Thread beside it (#1008, ADR 0140).
pub async fn register(
	scope: &mut dyn DutyScope,
	thread: Uuid,
	definition: Definition,
	origin: Origin,
) -> Result<Duty> {
	register_with(scope, thread, definition, origin, None).await
}

/// Sets up a Duty from a Marketplace template the person may install
/// (#1011 D4) from `thread`, as a Duty registered there: it opens its own
/// Thread beside it (#1008). The person who installs it is its Requester, as
/// for any Duty they set up, and it is staffed by `agent`, else by the Agent
/// the template suggests. Its time trigger is the template's; its other
/// triggers are the person's to create, since only they may hold a webhook
/// secret. The template version is recorded so a later one can be offered as
/// an update.
pub async fn install_template(
	scope: &mut dyn DutyScope,
	thread: Uuid,
	template: &Template,
	agent: Option<EntityRef>,
	source: Source,
) -> Result<Duty> {
	template.validate(scope.now())?;
	let agent = agent
		.or_else(|| template.agent.as_ref().map(|agent| agent.reference()))
		.ok_or_else(|| Error::Invalid("choose the Agent that carries the Duty out".into()))?;
	register_with(
		scope,
		thread,
		template.definition(agent),
		Origin::Person,
		Some(source),
	)
	.await
}

async fn register_with(
	scope: &mut dyn DutyScope,
	thread: Uuid,
	definition: Definition,
	origin: Origin,
	template: Option<Source>,
) -> Result<Duty> {
	scope.require_thread(thread, "thread.read").await?;
	if let Origin::Agent { run_id, key } = &origin
		&& let Some(existing) = scope.created_by(*run_id, key).await?
	{
		return Ok(existing);
	}
	scope.require_thread(thread, duties::CREATE).await?;
	let requester = scope.subject().to_owned();
	// A Duty always acts as one person (ADR 0134): a service or Agent
	// subject cannot be a Requester, however it authenticated.
	if !scope.is_person(&requester).await? {
		return Err(Error::Invalid(
			"a Duty acts as a person, and only a person of the Tenant can register one".into(),
		));
	}
	check_definition(scope, thread, &definition).await?;
	let total = scope.count().await?;
	duties::admit_within_tier(total, scope.tier_cap().await?)?;
	let now = scope.now();
	let own = scope.open_thread(thread, &definition).await?;
	let mut duty = Duty {
		id: Uuid::new_v4(),
		thread_id: own,
		origin_thread_id: Some(thread),
		requester,
		credential_id: scope.requester_credential().await?,
		definition,
		anchor: now,
		next_scheduled_at: None,
		watcher_next_at: None,
		halt: None,
		halted_at: None,
		halt_reason: None,
		successor: None,
		consecutive_failures: 0,
		created_by_run_id: match &origin {
			Origin::Person => None,
			Origin::Agent { run_id, .. } => Some(*run_id),
		},
		creation_key: match origin {
			Origin::Person => None,
			Origin::Agent { key, .. } => Some(key),
		},
		template,
		revision: 0,
		created_at: now,
		updated_at: now,
	};
	duty.restart(now)?;
	scope.insert(&duty).await?;
	Ok(duty)
}

async fn actor(scope: &mut dyn DutyScope) -> Result<(String, bool)> {
	let administers = scope.administers().await?;
	Ok((scope.subject().to_owned(), administers))
}

/// Loads a Duty the person can see, or reports it missing.
async fn visible(scope: &mut dyn DutyScope, id: Uuid, lock: bool) -> Result<Duty> {
	let duty = if lock {
		scope.load_for_update(id).await?
	} else {
		scope.load(id).await?
	};
	// The named successor sees the Duty they are asked to take over,
	// even without the Thread, so they can review and resume it.
	if duty.requester == scope.subject()
		|| duty.successor.as_deref() == Some(scope.subject())
		|| scope.sees_thread(duty.thread_id).await?
		|| match duty.origin_thread_id {
			Some(origin) => scope.sees_thread(origin).await?,
			None => false,
		} || scope.administers().await?
	{
		Ok(duty)
	} else {
		Err(Error::NotFound("duty not found".into()))
	}
}

async fn permitted(scope: &mut dyn DutyScope, duty: &Duty, change: Change) -> Result<()> {
	let (subject, administers) = actor(scope).await?;
	duties::permit(
		Actor {
			subject: &subject,
			administers,
		},
		duty.standing(),
		change,
	)?;
	Ok(())
}

/// Changes a Duty's settings. Only its Requester may, and only while it
/// holds `duty.create` there.
pub async fn update(scope: &mut dyn DutyScope, id: Uuid, patch: Patch) -> Result<Duty> {
	let mut duty = visible(scope, id, true).await?;
	permitted(scope, &duty, Change::Edit).await?;
	if patch.is_empty() {
		return Ok(duty);
	}
	change(scope, &mut duty, &patch).await?;
	scope.save(&duty).await?;
	Ok(duty)
}

/// Moves a Duty installed from a template to a later version of it (#1011
/// D4), as its Requester's own edit: the template's mission, procedure and
/// scripts replace the Duty's, and its title, Agent and triggers stay.
/// Nothing moves a Duty to a new version but this.
pub async fn update_from_template(
	scope: &mut dyn DutyScope,
	id: Uuid,
	template: &Template,
	source: Source,
) -> Result<Duty> {
	let mut duty = visible(scope, id, true).await?;
	permitted(scope, &duty, Change::Edit).await?;
	if !duty
		.template
		.as_ref()
		.is_some_and(|current| current.precedes(&source))
	{
		return Err(Error::Invalid(
			"only a later version of the template the Duty was installed from updates it".into(),
		));
	}
	template.validate(scope.now())?;
	change(scope, &mut duty, &template.update_patch()).await?;
	duty.template = Some(Source {
		install_key: duty
			.template
			.as_ref()
			.and_then(|current| current.install_key),
		..source
	});
	scope.save(&duty).await?;
	Ok(duty)
}

/// A Duty of the person's own, for them to publish as a template (#1011
/// D4). Only its Requester publishes a Duty: its mission and scripts are
/// theirs to share.
pub async fn own(scope: &mut dyn DutyScope, id: Uuid) -> Result<Duty> {
	let duty = visible(scope, id, false).await?;
	if duty.requester != scope.subject() {
		return Err(Error::Invalid(
			"only the Duty's Requester can publish it as a template".into(),
		));
	}
	Ok(duty)
}

/// Applies `patch` to a loaded Duty as its Requester's edit; the caller saves.
async fn change(scope: &mut dyn DutyScope, duty: &mut Duty, patch: &Patch) -> Result<()> {
	scope.require_thread(duty.thread_id, duties::CREATE).await?;
	let (definition, retimed) = patch.apply(&duty.definition);
	check_definition(scope, duty.thread_id, &definition).await?;
	let rewatched = definition.watcher != duty.definition.watcher;
	duty.definition = definition;
	let now = scope.now();
	if retimed && duty.halt.is_none() {
		duty.restart(now)?;
	} else if rewatched && duty.halt.is_none() {
		// A changed Watcher runs at the next pass, a removed one never again.
		duty.rewatch(now);
	} else if rewatched {
		duty.watcher_next_at = None;
	}
	duty.updated_at = now;
	Ok(())
}

/// A Duty its Requester may change: for adding or removing its triggers.
pub async fn editable(scope: &mut dyn DutyScope, id: Uuid) -> Result<Duty> {
	let duty = visible(scope, id, true).await?;
	permitted(scope, &duty, Change::Edit).await?;
	Ok(duty)
}

pub async fn pause(scope: &mut dyn DutyScope, id: Uuid) -> Result<Duty> {
	let mut duty = visible(scope, id, true).await?;
	permitted(scope, &duty, Change::Pause).await?;
	if duty.halt.is_some() {
		return Ok(duty);
	}
	let now = scope.now();
	duty.stop(Halt::Paused, now);
	duty.updated_at = now;
	scope.save(&duty).await?;
	Ok(duty)
}

/// Resumes a Duty as the person's own action. A named successor takes
/// it over and becomes its Requester; nobody else resumes one whose
/// Requester left (ADR 0134).
pub async fn resume(scope: &mut dyn DutyScope, id: Uuid) -> Result<Duty> {
	let mut duty = visible(scope, id, true).await?;
	let (subject, administers) = actor(scope).await?;
	let resumption = duties::resumption(
		Actor {
			subject: &subject,
			administers,
		},
		duty.standing(),
	)?;
	// A successor takes the Duty, and its own Thread, over first: their
	// rights there are then judged as its owner's.
	if resumption == Resumption::Successor {
		scope.hand_over(&duty, &subject).await?;
	}
	// Resuming is registering again: the person must still be allowed to,
	// within the limits, with what the Duty names.
	scope.require_thread(duty.thread_id, duties::CREATE).await?;
	check_definition(scope, duty.thread_id, &duty.definition).await?;
	if resumption == Resumption::Successor {
		duty.requester = subject;
		duty.created_by_run_id = None;
	}
	let now = scope.now();
	// The person resumes it as their own action, with their Mapping's
	// credential as it stands now (ADR 0134).
	duty.credential_id = scope.requester_credential().await?;
	duty.clear_halt();
	duty.consecutive_failures = 0;
	duty.restart(now)?;
	duty.updated_at = now;
	scope.save(&duty).await?;
	Ok(duty)
}

/// Deletes a Duty, as a person. Its Budgets go with it, so one someone else
/// set needs `budget.manage` (#1012 decision 25).
pub async fn delete(scope: &mut dyn DutyScope, id: Uuid) -> Result<()> {
	delete_as(scope, id, Deleter::Person).await
}

/// `duty_delete` by an Agent: an Agent never removes a Budget, so it only
/// pauses a Duty a Budget targets (#1012 decision 25).
pub async fn delete_by_agent(scope: &mut dyn DutyScope, id: Uuid) -> Result<()> {
	delete_as(scope, id, Deleter::Agent).await
}

async fn delete_as(scope: &mut dyn DutyScope, id: Uuid, deleter: Deleter) -> Result<()> {
	let duty = visible(scope, id, true).await?;
	permitted(scope, &duty, Change::Delete).await?;
	let setters = scope.budget_setters(id).await?;
	if duties::budget_removal(deleter, scope.subject(), &setters)?
		== BudgetRemoval::NeedsBudgetManage
		&& !scope.manages_budgets().await?
	{
		return Err(Error::Invalid(duties::BUDGET_SET_BY_SOMEONE_ELSE.into()));
	}
	scope.delete(id).await
}

/// Names who may take over a Duty whose Requester left.
pub async fn name_successor(scope: &mut dyn DutyScope, id: Uuid, successor: &str) -> Result<Duty> {
	let mut duty = visible(scope, id, true).await?;
	permitted(scope, &duty, Change::NameSuccessor).await?;
	if !scope.is_person(successor).await? {
		return Err(Error::Invalid(
			"a successor must be a person of the Tenant".into(),
		));
	}
	duty.successor = Some(successor.to_owned());
	duty.updated_at = scope.now();
	scope.save(&duty).await?;
	Ok(duty)
}

pub async fn list(scope: &mut dyn DutyScope, listing: Listing) -> Result<Vec<Duty>> {
	match listing {
		Listing::Thread(thread) => {
			scope.require_thread(thread, "thread.read").await?;
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

pub async fn detail(scope: &mut dyn DutyScope, id: Uuid) -> Result<Detail> {
	let duty = visible(scope, id, false).await?;
	let ticks = scope.ticks(id, HISTORY_LIMIT).await?;
	let newer_agent_version = scope.newer_agent_version(&duty.definition.agent).await?;
	let (subject, administers) = actor(scope).await?;
	let may_edit = duties::permit(
		Actor {
			subject: &subject,
			administers,
		},
		duty.standing(),
		Change::Edit,
	)
	.is_ok();
	Ok(Detail {
		duty,
		ticks,
		newer_agent_version,
		may_edit,
		may_administer: administers,
	})
}

/// What an Agent sends to the Duty tools. The Duty is set up for the person
/// at the root of the Run's chain, in the Run's Thread.
pub mod tool {
	use aidash_domain::{
		approval_rules::ApprovalMode,
		duties::{ModeChoice, Trigger, TriggerChoice},
		registry::EntityRef,
	};
	use schemars::JsonSchema;
	use serde::Deserialize;
	use uuid::Uuid;

	#[derive(Debug, Clone, Deserialize, JsonSchema)]
	#[serde(deny_unknown_fields)]
	pub struct Create {
		/// A short name people see in the Duty list.
		pub title: String,
		/// What the Duty is for, written as a person would ask it; every Task
		/// it starts is asked this.
		pub mission: String,
		/// The checklist the Agent follows on every Task, if any.
		#[serde(default)]
		pub procedure: Option<String>,
		/// The Agent version to start; this Run's own Agent when omitted.
		#[serde(default)]
		pub agent: Option<EntityRef>,
		/// When it wakes by the clock:
		/// `{"kind":"cron","expression":"0 9 * * 1-5","time_zone":"Asia/Tokyo"}`
		/// (minute hour day-of-month month day-of-week) or
		/// `{"kind":"interval","every_seconds":3600}`. Omit it for a Duty that
		/// wakes only on its other triggers.
		#[serde(default)]
		pub trigger: Option<Trigger>,
		/// The Approval Mode of its Runs; the person's own default when omitted.
		#[serde(default)]
		pub approval_mode: Option<ApprovalMode>,
		/// How late, in seconds, a tick missed while Aidash was down may still run.
		#[serde(default)]
		pub max_lateness_seconds: Option<i64>,
		/// Failed Tasks in a row after which the Duty pauses itself; 0 never
		/// pauses it. 3 when omitted.
		#[serde(default)]
		pub pause_after_failures: Option<i32>,
	}

	#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
	#[serde(deny_unknown_fields)]
	pub struct List {}

	#[derive(Debug, Clone, Deserialize, JsonSchema)]
	#[serde(deny_unknown_fields)]
	pub struct Update {
		pub duty_id: Uuid,
		#[serde(default)]
		pub title: Option<String>,
		#[serde(default)]
		pub mission: Option<String>,
		#[serde(default)]
		pub procedure: Option<String>,
		#[serde(default)]
		pub agent: Option<EntityRef>,
		/// A new time trigger, or `{"kind":"none"}` to remove it.
		#[serde(default)]
		pub trigger: Option<TriggerChoice>,
		/// `default` returns to the person's own default mode.
		#[serde(default)]
		pub approval_mode: Option<ModeChoice>,
		#[serde(default)]
		pub max_lateness_seconds: Option<i64>,
		#[serde(default)]
		pub pause_after_failures: Option<i32>,
	}

	/// What `duty_delete` does.
	#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
	#[serde(rename_all = "snake_case")]
	pub enum Removal {
		#[default]
		Delete,
		/// Keep the Duty but stop it firing until the person resumes it.
		Pause,
	}

	#[derive(Debug, Clone, Deserialize, JsonSchema)]
	#[serde(deny_unknown_fields)]
	pub struct Delete {
		pub duty_id: Uuid,
		#[serde(default)]
		pub action: Removal,
	}

	/// An event for a Duty on another Node (#1013). Its Requester set up a
	/// `node_event` trigger and gave you the Node and the trigger's id.
	#[derive(Debug, Clone, Deserialize, JsonSchema)]
	#[serde(deny_unknown_fields)]
	pub struct Notify {
		/// The address of the Node the Duty is on, such as
		/// `aidash://deployment/quiet-falcon-4f8k2m1z`.
		pub node_id: String,
		/// The id of the Duty's `node_event` trigger.
		pub trigger_id: Uuid,
		/// The event: JSON of at most 256 KiB. The Duty reads it as data.
		#[serde(default)]
		pub payload: serde_json::Value,
	}
}

#[cfg(test)]
mod tests;
