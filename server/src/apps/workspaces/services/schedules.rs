//! Schedules (#557, ADR 0134): authorized operations for people and Agents,
//! the scheduler pass every replica runs, and the notices the bell shows.
use super::access::Lease;
use crate::apps::workspaces::repositories::schedules::{self as repository, Filter, Stop};
use crate::apps::workspaces::serializers::schedules::{
	DetailView, LimitsView, NoticesView, ScheduleNotice, ScheduleView,
};
use crate::{
	Error, Result,
	authorization::{
		access::Access,
		identity::{Actor, SubjectIdentity},
	},
	federation::Federation,
	store::Store,
};
use aidash_application::schedules::{self as app, Listing, Origin, ScheduleScope};
use aidash_domain::{
	approval_rules::{ApprovalMode, MODE_RESOURCE},
	policy::SubjectKind,
	registry::{EntityRef, Search},
	schedules::{
		self, Ceilings, Claim, Definition, Halt, Limits, Patch, Schedule, ThreadMode, Tick,
		TickStatus,
	},
	tenancy::TenantName,
};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

/// The person acting in one Schedule transaction.
pub(crate) struct Adapter<'a> {
	pub access: &'a mut Access,
	pub store: &'a Store,
	pub now: DateTime<Utc>,
}

fn tenant(access: &Access) -> String {
	access.identity.tenant.clone()
}

#[async_trait::async_trait]
impl ScheduleScope for Adapter<'_> {
	fn subject(&self) -> &str {
		&self.access.identity.subject
	}
	fn credential(&self) -> Uuid {
		self.access.identity.credential_id
	}
	fn now(&self) -> DateTime<Utc> {
		self.now
	}
	async fn require_workspace(
		&mut self,
		workspace: Uuid,
		action: &str,
	) -> aidash_application::Result<()> {
		let resource = self.access.workspace(workspace).await?;
		self.access.require(&resource, action).await?;
		Ok(())
	}
	async fn sees_workspace(&mut self, workspace: Uuid) -> aidash_application::Result<bool> {
		let resource = match self.access.workspace(workspace).await {
			Ok(resource) => resource,
			Err(Error::NotFound(_) | Error::Forbidden) => return Ok(false),
			Err(error) => return Err(error.into()),
		};
		Ok(self.access.decide(&resource, "workspace.read").await?)
	}
	async fn administers(&mut self) -> aidash_application::Result<bool> {
		let resource = self
			.access
			.resource("tenant", tenant(self.access), json!({}));
		Ok(self.access.decide(&resource, schedules::ADMINISTER).await?)
	}
	async fn mode_allowed(&mut self, mode: ApprovalMode) -> aidash_application::Result<bool> {
		if let Some((action, id)) = mode.policy_requirement() {
			let target = self.access.resource(MODE_RESOURCE, id, json!({}));
			if !self.access.decide(&target, action).await? {
				return Ok(false);
			}
		}
		if mode == ApprovalMode::Bypass {
			let (tenant, subject) = (tenant(self.access), self.subject().to_owned());
			let preference = crate::apps::identity::repositories::approval_preferences::preference(
				&mut *self.access.tx,
				&tenant,
				&subject,
			)
			.await?;
			// As for a Thread's mode: the person accepted the risk of Bypass first.
			return Ok(preference.is_some_and(|row| row.bypass_accepted_at.is_some()));
		}
		Ok(true)
	}
	async fn require_agent(&mut self, agent: &EntityRef) -> aidash_application::Result<()> {
		crate::apps::identity::services::catalog::entry(self.access, agent, "agent.execute")
			.await?;
		Ok(())
	}
	async fn newer_agent_version(
		&mut self,
		agent: &EntityRef,
	) -> aidash_application::Result<Option<String>> {
		let Ok(current) = semver::Version::parse(&agent.version) else {
			return Ok(None);
		};
		let entries = crate::apps::identity::services::catalog::list_in(
			self.access,
			&Search {
				kind: Some("agent".into()),
				..Search::default()
			},
		)
		.await?;
		Ok(entries
			.into_iter()
			.filter(|entry| entry.id == agent.id)
			.filter_map(|entry| semver::Version::parse(&entry.version).ok())
			.filter(|version| *version > current)
			.max()
			.map(|version| version.to_string()))
	}
	async fn thread_in_workspace(
		&mut self,
		workspace: Uuid,
		thread: Uuid,
	) -> aidash_application::Result<bool> {
		match crate::apps::execution::capabilities::services::endpoints::readable_thread(
			self.access,
			workspace,
			thread,
		)
		.await
		{
			Ok(()) => Ok(true),
			Err(Error::NotFound(_) | Error::Forbidden) => Ok(false),
			Err(error) => Err(error.into()),
		}
	}
	async fn is_person(&mut self, subject: &str) -> aidash_application::Result<bool> {
		Ok(self
			.access
			.snapshot
			.bundle
			.subjects
			.get(subject)
			.is_some_and(|subject| subject.enabled && subject.kind == SubjectKind::User))
	}
	async fn limits(&mut self) -> aidash_application::Result<(Limits, Ceilings, Option<u32>)> {
		let tenant = tenant(self.access);
		let limits = repository::limits(&mut *self.access.tx, &tenant).await?;
		let tier = self
			.store
			.schedule_tier
			.schedules(&TenantName::new(&tenant)?)
			.await?;
		Ok((limits, self.store.schedule_ceilings, tier))
	}
	async fn save_limits(&mut self, limits: &Limits) -> aidash_application::Result<()> {
		let (tenant, subject) = (tenant(self.access), self.subject().to_owned());
		repository::put_limits(&mut *self.access.tx, &tenant, &subject, limits, self.now).await?;
		Ok(())
	}
	async fn counts(&mut self, registrant: &str) -> aidash_application::Result<(i64, i64)> {
		let tenant = tenant(self.access);
		Ok(repository::counts(&mut *self.access.tx, &tenant, registrant).await?)
	}
	async fn insert(&mut self, schedule: &Schedule) -> aidash_application::Result<()> {
		let tenant = tenant(self.access);
		Ok(repository::insert(&mut *self.access.tx, &tenant, schedule).await?)
	}
	async fn load_for_update(&mut self, id: Uuid) -> aidash_application::Result<Schedule> {
		let tenant = tenant(self.access);
		repository::load(&mut *self.access.tx, &tenant, id, true)
			.await?
			.ok_or_else(|| aidash_application::Error::NotFound("schedule not found".into()))
	}
	async fn load(&mut self, id: Uuid) -> aidash_application::Result<Schedule> {
		let tenant = tenant(self.access);
		repository::load(&mut *self.access.tx, &tenant, id, false)
			.await?
			.ok_or_else(|| aidash_application::Error::NotFound("schedule not found".into()))
	}
	async fn save(&mut self, schedule: &Schedule) -> aidash_application::Result<()> {
		let tenant = tenant(self.access);
		repository::save(&mut *self.access.tx, &tenant, schedule).await?;
		Ok(())
	}
	async fn delete(&mut self, id: Uuid) -> aidash_application::Result<()> {
		let tenant = tenant(self.access);
		Ok(repository::delete(&mut *self.access.tx, &tenant, id).await?)
	}
	async fn list(&mut self, listing: Listing) -> aidash_application::Result<Vec<Schedule>> {
		let (tenant, subject) = (tenant(self.access), self.subject().to_owned());
		let filter = match listing {
			Listing::Workspace(workspace) => Filter::Workspace(workspace),
			Listing::Mine => Filter::Registrant(&subject),
			Listing::Tenant => Filter::Tenant,
		};
		Ok(repository::list(&mut *self.access.tx, &tenant, filter).await?)
	}
	async fn ticks(&mut self, id: Uuid, limit: u32) -> aidash_application::Result<Vec<Tick>> {
		let tenant = tenant(self.access);
		Ok(repository::ticks(&mut *self.access.tx, &tenant, id, limit).await?)
	}
}

/// What a person asks of the Schedules API.
pub(crate) enum Operation {
	Create(Uuid, Definition),
	List(Listing),
	Read(Uuid),
	Update(Uuid, Patch),
	Pause(Uuid),
	Resume(Uuid),
	Delete(Uuid),
	Successor(Uuid, String),
	RunNow(Uuid),
	Limits,
	SetLimits(Limits),
	Notices,
}

/// Runs one person's operation in one transaction.
pub(crate) async fn execute(f: &Federation, actor: Actor, op: Operation) -> Result<Value> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	if let Operation::RunNow(id) = op {
		return run_now(f, &identity, id).await;
	}
	let mut access = Access::begin(&f.store, &identity).await?;
	let now = Utc::now();
	let result: Result<Value> = async {
		let mut scope = Adapter {
			access: &mut access,
			store: &f.store,
			now,
		};
		let mine = scope.subject().to_owned();
		let view = |schedule: Schedule| ScheduleView::new(schedule, &mine);
		Ok(match op {
			Operation::Create(workspace, definition) => serde_json::to_value(view(
				app::register(&mut scope, workspace, definition, Origin::Person).await?,
			))?,
			Operation::List(listing) => serde_json::to_value(
				app::list(&mut scope, listing)
					.await?
					.into_iter()
					.map(view)
					.collect::<Vec<_>>(),
			)?,
			Operation::Read(id) => {
				serde_json::to_value(DetailView::new(app::detail(&mut scope, id).await?, &mine))?
			}
			Operation::Update(id, patch) => {
				serde_json::to_value(view(app::update(&mut scope, id, patch).await?))?
			}
			Operation::Pause(id) => serde_json::to_value(view(app::pause(&mut scope, id).await?))?,
			Operation::Resume(id) => {
				serde_json::to_value(view(app::resume(&mut scope, id).await?))?
			}
			Operation::Delete(id) => {
				app::delete(&mut scope, id).await?;
				json!({"deleted": id})
			}
			Operation::Successor(id, successor) => {
				serde_json::to_value(view(app::name_successor(&mut scope, id, &successor).await?))?
			}
			Operation::Limits => {
				let (limits, ceilings) = app::limits(&mut scope).await?;
				serde_json::to_value(LimitsView { limits, ceilings })?
			}
			Operation::SetLimits(limits) => {
				let limits = app::set_limits(&mut scope, limits).await?;
				serde_json::to_value(LimitsView {
					limits,
					ceilings: f.store.schedule_ceilings,
				})?
			}
			Operation::Notices => serde_json::to_value(notices(&mut scope).await?)?,
			Operation::RunNow(_) => unreachable!("handled above"),
		})
	}
	.await;
	access.finish(result).await
}

/// The Schedules the bell tells this person about (#557 decision 25): their
/// own that stopped without anyone choosing it, those waiting for them as a
/// named successor, and, for holders of `schedule.administer`, those waiting
/// for a successor. Also the Tasks their Schedules started this week, whose
/// completion the bell shows without a toast (decision 26).
async fn notices(scope: &mut Adapter<'_>) -> Result<NoticesView> {
	let administers = scope.administers().await?;
	let subject = scope.subject().to_owned();
	let tenant = tenant(scope.access);
	let all = if administers {
		repository::list(&mut *scope.access.tx, &tenant, Filter::Tenant).await?
	} else {
		let mut mine =
			repository::list(&mut *scope.access.tx, &tenant, Filter::Registrant(&subject)).await?;
		mine.extend(
			repository::list(&mut *scope.access.tx, &tenant, Filter::Tenant)
				.await?
				.into_iter()
				.filter(|schedule| schedule.successor.as_deref() == Some(subject.as_str())),
		);
		mine
	};
	let stopped = all
		.iter()
		.filter(|schedule| {
			schedule.halt.is_some_and(|halt| {
				halt.notifies()
					&& (schedule.registrant == subject
						|| schedule.successor.as_deref() == Some(subject.as_str())
						|| (administers && halt == Halt::RegistrantLeft))
			})
		})
		.map(ScheduleNotice::of)
		.collect();
	let own: Vec<Uuid> = all
		.iter()
		.filter(|schedule| schedule.registrant == subject)
		.map(|schedule| schedule.id)
		.collect();
	let scheduled_tasks = repository::recent_tasks(
		&mut *scope.access.tx,
		&tenant,
		&own,
		scope.now - Duration::days(7),
	)
	.await?;
	Ok(NoticesView {
		stopped,
		scheduled_tasks,
	})
}

/// A stable key for a tick, so a retried start reuses its message and Run.
fn tick_key(schedule: Uuid, scheduled_at: DateTime<Utc>, part: &str) -> Uuid {
	let digest = crate::registry::digest(&json!(["schedule_tick", schedule, scheduled_at, part]));
	let mut bytes = [0_u8; 16];
	for (index, byte) in bytes.iter_mut().enumerate() {
		*byte = u8::from_str_radix(&digest[index * 2..index * 2 + 2], 16).unwrap_or_default();
	}
	uuid::Builder::from_random_bytes(bytes).into_uuid()
}

/// Starts one Run of `schedule` as the lease's subject: in a new Thread whose
/// first message is the request, or in its fixed Thread. Returns the Task and
/// the Thread.
async fn start(
	f: &Federation,
	lease: &mut Lease,
	schedule: &Schedule,
	key: (DateTime<Utc>, &str),
) -> Result<(Uuid, Uuid)> {
	let workspace = schedule.workspace_id;
	let definition = &schedule.definition;
	let thread = match definition.thread {
		ThreadMode::Fixed { thread_id } => thread_id,
		ThreadMode::New => {
			let resource = lease.access_mut().workspace(workspace).await?;
			lease
				.access_mut()
				.require(&resource, "message.create")
				.await?;
			let message = super::threads::post(
				&f.store,
				Some(f),
				lease,
				workspace,
				crate::apps::workspaces::serializers::channels::ChannelMessageInput {
					content: definition.request.clone(),
					thread_id: None,
					idempotency_key: tick_key(schedule.id, key.0, &format!("{}:message", key.1)),
					attachment_ids: vec![],
					file_versions: vec![],
				},
			)
			.await?;
			super::threads::create(&f.store, lease, workspace, message.message.id)
				.await?
				.id
		}
	};
	if let Some(mode) = definition.approval_mode {
		// The Schedule's Approval Mode is its Thread's mode for the person
		// its Runs act as (#557 decision 4).
		let access = lease.access_mut();
		let (tenant, subject) = (tenant(access), access.identity.subject.clone());
		crate::apps::identity::repositories::approval_preferences::lock_owner(
			&mut *access.tx,
			&tenant,
			&subject,
		)
		.await?;
		crate::apps::identity::repositories::approval_preferences::set_thread_mode(
			&mut *access.tx,
			&tenant,
			&subject,
			thread,
			Some(mode),
		)
		.await?;
	}
	let (_, task) = crate::apps::execution::capabilities::services::endpoints::start_in_thread(
		f,
		lease.access_mut(),
		(workspace, thread, definition.agent.id.clone()),
		crate::apps::execution::capabilities::serializers::contracts::Enqueue {
			idempotency_key: tick_key(schedule.id, key.0, &format!("{}:run", key.1)),
			agent_version: definition.agent.version.clone(),
			title: definition.title.clone(),
			description: definition.request.clone(),
			read_pages: vec![],
		},
	)
	.await?;
	let task = task.ok_or_else(|| Error::Conflict("this tick already started".into()))?;
	Ok((task, thread))
}

/// "Run now": an ordinary Run of the Schedule as the person who clicked it,
/// recorded in its history (#557 decision 24).
async fn run_now(f: &Federation, identity: &SubjectIdentity, id: Uuid) -> Result<Value> {
	let access = Access::begin(&f.store, identity).await?;
	let mut lease = Lease::from_access(access);
	let now = Utc::now();
	let result: Result<Value> = async {
		let tenant = identity.tenant.clone();
		let schedule = repository::load(lease.tx(), &tenant, id, false)
			.await?
			.ok_or_else(|| Error::NotFound("schedule not found".into()))?;
		let resource = lease.access_mut().workspace(schedule.workspace_id).await?;
		if !lease
			.access_mut()
			.decide(&resource, "workspace.read")
			.await?
		{
			return Err(Error::NotFound("schedule not found".into()));
		}
		let (task, thread) = start(f, &mut lease, &schedule, (now, &identity.subject)).await?;
		repository::insert_tick(
			lease.tx(),
			&tenant,
			schedule.workspace_id,
			&Tick {
				id: Uuid::new_v4(),
				schedule_id: schedule.id,
				scheduled_at: now,
				recorded_at: now,
				status: TickStatus::Started,
				reason: None,
				task_id: Some(task),
				thread_id: Some(thread),
				started_by: Some(identity.subject.clone()),
			},
		)
		.await?;
		Ok(json!({"task_id": task, "thread_id": thread}))
	}
	.await;
	let value = lease.finish(result).await?;
	f.notify.notify_waiters();
	Ok(value)
}

/// What one scheduler pass did, for logs and tests.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Pass {
	pub started: Vec<Uuid>,
	pub skipped: Vec<(Uuid, String)>,
	pub failed: Vec<(Uuid, String)>,
	pub stopped: Vec<(Uuid, Halt)>,
	pub settled: usize,
}

/// Due Schedules handled per pass; the rest wait for the next one.
const BATCH: u64 = 32;

/// One pass of the scheduler on this replica: settle the ticks whose Runs
/// ended, then handle the due Schedules no other replica holds. Every replica
/// runs it; `SKIP LOCKED` and the tick key keep a tick to one Task.
pub async fn pass(f: &Federation, now: DateTime<Utc>) -> Result<Pass> {
	let mut report = Pass::default();
	for ended in repository::ended(&f.store.pool, 256).await? {
		match settle(f, &ended, now).await {
			Ok(true) => report.settled += 1,
			Ok(false) => {}
			Err(error) => {
				tracing::warn!(%error, tick = %ended.tick, "a Schedule tick could not be settled");
			}
		}
	}
	for (tenant, id) in repository::due(&f.store.pool, now, BATCH).await? {
		if let Err(error) = fire(f, &tenant, id, now, &mut report).await {
			tracing::warn!(%error, schedule = %id, "a due Schedule could not be handled");
		}
	}
	if !report.started.is_empty() {
		f.notify.notify_waiters();
	}
	Ok(report)
}

/// Settles one ended tick and counts a failure toward the Schedule's pause.
async fn settle(f: &Federation, ended: &repository::Ended, now: DateTime<Utc>) -> Result<bool> {
	let scope = aidash_application::tenancy::TenancyScope::tenant(&ended.tenant)?;
	let mut tx = f.store.pool.begin(&scope).await?;
	let result: Result<bool> = async {
		if !repository::settle_tick(&mut tx, &ended.tenant, ended.tick, ended.status, now).await? {
			return Ok(false);
		}
		if ended.manual {
			return Ok(true);
		}
		let Some(mut schedule) =
			repository::load(&mut tx, &ended.tenant, ended.schedule, true).await?
		else {
			return Ok(true);
		};
		let (failures, pause) =
			schedules::after_settled(schedule.consecutive_failures, ended.status);
		if failures == schedule.consecutive_failures && !pause {
			return Ok(true);
		}
		schedule.consecutive_failures = failures;
		if pause && schedule.halt.is_none() {
			schedule.stop(Halt::Failures, now);
		}
		schedule.updated_at = now;
		repository::save(&mut tx, &ended.tenant, &schedule).await?;
		Ok(true)
	}
	.await;
	match result {
		Ok(value) => {
			tx.commit().await?;
			Ok(value)
		}
		Err(error) => {
			tx.rollback().await?;
			Err(error)
		}
	}
}

/// Why a claimed tick did not start.
enum Miss {
	/// Recorded as skipped with this reason; it does not count as a failure.
	Skip(String),
	/// Recorded as failed; it counts toward the pause.
	Fail(String),
	/// The Schedule stops for this reason, and the tick is recorded as failed.
	Stop(Halt, String),
}

/// What became of a claimed tick that started nothing.
fn classify(error: &Error) -> Miss {
	match error {
		// The usage gate refused it, as it would a Run started by hand.
		Error::Refused(code) => Miss::Skip(format!("{}: {code}", schedules::skip::LIMIT)),
		other => Miss::Fail(other.to_string()),
	}
}

/// What a claimed tick did in its Registrant's transaction.
enum Fired {
	/// Another replica held it, or it was no longer due.
	Nothing,
	Skipped(&'static str),
	Started(Uuid),
}

/// Handles one due Schedule as its Registrant: claim, then start, skip or stop.
async fn fire(
	f: &Federation,
	tenant: &str,
	id: Uuid,
	now: DateTime<Utc>,
	report: &mut Pass,
) -> Result<()> {
	// Read who it acts as, then act as them; the claim below rechecks the row.
	let scope = aidash_application::tenancy::TenancyScope::tenant(tenant)?;
	let Some(schedule) = f
		.store
		.pool
		.scoped(&scope, async |tx| {
			repository::load(&mut **tx, tenant, id, false).await
		})
		.await?
	else {
		return Ok(());
	};
	let identity = SubjectIdentity {
		http_session: None,
		credential_id: schedule.credential_id,
		tenant: tenant.to_owned(),
		subject: schedule.registrant.clone(),
	};
	let access = match Access::begin(&f.store, &identity).await {
		Ok(access) => access,
		// The Registrant left the Tenant or lost the credential it acts with.
		Err(Error::Unauthorized | Error::Forbidden) => {
			return record_miss(
				f,
				tenant,
				&schedule,
				None,
				now,
				Miss::Stop(Halt::RegistrantLeft, "the Registrant lost access".into()),
				report,
			)
			.await;
		}
		Err(error) => return Err(error),
	};
	let mut lease = Lease::from_access(access);
	let mut claimed: Option<Claim> = None;
	// Set when the tick shows the Schedule must stop, before the start rolls back.
	let mut stopping: Option<(Halt, String)> = None;
	let result: Result<Fired> = async {
		let Some(schedule) = repository::claim(lease.tx(), tenant, id, now).await? else {
			return Ok(Fired::Nothing);
		};
		let Some(due) = schedule.next_scheduled_at else {
			return Ok(Fired::Nothing);
		};
		let Some(claim) = schedule.timing().claim(due, now)? else {
			return Ok(Fired::Nothing);
		};
		claimed = Some(claim.clone());
		let skip = if claim.late {
			Some(schedules::skip::LATE)
		} else if repository::running(lease.tx(), tenant, id).await? {
			Some(schedules::skip::OVERLAP)
		} else {
			// The minimum interval holds at run time too: a daylight-saving
			// change can bring two scheduled times closer than registration saw.
			let limits = repository::limits(lease.tx(), tenant).await?;
			let min_interval = f
				.store
				.schedule_ceilings
				.effective(limits, None)
				.min_interval_seconds;
			let previous = repository::last_scheduled(lease.tx(), tenant, id).await?;
			schedules::too_soon(previous, claim.scheduled_at, min_interval)
				.then_some(schedules::skip::INTERVAL)
		};
		if let Some(reason) = skip {
			advance(lease.tx(), tenant, &schedule, &claim, now).await?;
			skipped_tick(lease.tx(), tenant, &schedule, &claim, reason, now).await?;
			return Ok(Fired::Skipped(reason));
		}
		// The Registrant must still be able to register it and start Tasks
		// in its Workspace; otherwise they lost access there.
		let resource = lease.access_mut().workspace(schedule.workspace_id).await?;
		for action in ["workspace.read", "task.create", schedules::CREATE] {
			if !lease.access_mut().decide(&resource, action).await? {
				stopping = Some((
					Halt::RegistrantLeft,
					format!("the Registrant may no longer {action} in this Workspace"),
				));
				return Err(Error::Forbidden);
			}
		}
		// Only a refusal stops the Schedule; any other error, such as a
		// database outage, leaves the tick to the next pass.
		if let ThreadMode::Fixed { thread_id } = schedule.definition.thread {
			match crate::apps::execution::capabilities::services::endpoints::readable_thread(
				lease.access_mut(),
				schedule.workspace_id,
				thread_id,
			)
			.await
			{
				Ok(()) => {}
				Err(error @ (Error::NotFound(_) | Error::Forbidden)) => {
					stopping = Some((
						Halt::ThreadDeleted,
						"the Schedule's Thread was deleted".into(),
					));
					return Err(error);
				}
				Err(error) => return Err(error),
			}
		}
		match crate::apps::identity::services::catalog::entry(
			lease.access_mut(),
			&schedule.definition.agent,
			"agent.execute",
		)
		.await
		{
			Ok(_) => {}
			Err(error @ (Error::NotFound(_) | Error::Forbidden)) => {
				let reason = format!(
					"Agent {}@{} can no longer be started",
					schedule.definition.agent.id, schedule.definition.agent.version
				);
				stopping = Some((Halt::AgentVersionUnusable, reason));
				return Err(error);
			}
			Err(error) => return Err(error),
		}
		let (task, thread) = start(f, &mut lease, &schedule, (claim.scheduled_at, "")).await?;
		advance(lease.tx(), tenant, &schedule, &claim, now).await?;
		repository::insert_tick(
			lease.tx(),
			tenant,
			schedule.workspace_id,
			&Tick {
				id: Uuid::new_v4(),
				schedule_id: schedule.id,
				scheduled_at: claim.scheduled_at,
				recorded_at: now,
				status: TickStatus::Started,
				reason: None,
				task_id: Some(task),
				thread_id: Some(thread),
				started_by: None,
			},
		)
		.await?;
		Ok(Fired::Started(task))
	}
	.await;
	let outcome = match result {
		Ok(fired) => {
			lease.finish(Ok(())).await?;
			match fired {
				Fired::Started(task) => report.started.push(task),
				Fired::Skipped(reason) => report.skipped.push((id, reason.into())),
				Fired::Nothing => {}
			}
			return Ok(());
		}
		Err(error) => lease.finish::<()>(Err(error)).await.unwrap_err(),
	};
	let Some(claim) = claimed else {
		return Err(outcome);
	};
	let miss = match stopping {
		Some((halt, reason)) => Miss::Stop(halt, reason),
		None => classify(&outcome),
	};
	record_miss(f, tenant, &schedule, Some(&claim), now, miss, report).await
}

/// Moves a claimed Schedule past the tick it handled.
async fn advance(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	schedule: &Schedule,
	claim: &Claim,
	now: DateTime<Utc>,
) -> Result<()> {
	let mut next = schedule.clone();
	next.next_scheduled_at = claim.next;
	next.updated_at = now;
	repository::save(&mut *tx, tenant, &next).await?;
	Ok(())
}

async fn skipped_tick(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	schedule: &Schedule,
	claim: &Claim,
	reason: &str,
	now: DateTime<Utc>,
) -> Result<()> {
	repository::insert_tick(
		&mut *tx,
		tenant,
		schedule.workspace_id,
		&Tick {
			id: Uuid::new_v4(),
			schedule_id: schedule.id,
			scheduled_at: claim.scheduled_at,
			recorded_at: now,
			status: TickStatus::Skipped,
			reason: Some(reason.to_owned()),
			task_id: None,
			thread_id: None,
			started_by: None,
		},
	)
	.await
}

/// Records a tick that started nothing, in a transaction of its own after the
/// start rolled back. It is never retried silently: a refused start is a
/// failed or skipped tick with its reason (#557, carried from #540).
async fn record_miss(
	f: &Federation,
	tenant: &str,
	read: &Schedule,
	claim: Option<&Claim>,
	now: DateTime<Utc>,
	miss: Miss,
	report: &mut Pass,
) -> Result<()> {
	let scope = aidash_application::tenancy::TenancyScope::tenant(tenant)?;
	let mut tx = f.store.pool.begin(&scope).await?;
	let result: Result<()> = async {
		let Some(mut schedule) = repository::load(&mut tx, tenant, read.id, true).await? else {
			return Ok(());
		};
		// Another replica handled this tick meanwhile, or a person changed it.
		if schedule.halt.is_some() || schedule.next_scheduled_at != read.next_scheduled_at {
			return Ok(());
		}
		let scheduled_at = claim.map_or(now, |claim| claim.scheduled_at);
		let (status, reason) = match &miss {
			Miss::Skip(reason) => (TickStatus::Skipped, reason.clone()),
			Miss::Fail(reason) | Miss::Stop(_, reason) => (TickStatus::Failed, reason.clone()),
		};
		if let Some(claim) = claim {
			schedule.next_scheduled_at = claim.next;
		}
		match miss {
			Miss::Skip(reason) => report.skipped.push((schedule.id, reason)),
			Miss::Fail(reason) => {
				let (failures, pause) =
					schedules::after_settled(schedule.consecutive_failures, TickStatus::Failed);
				schedule.consecutive_failures = failures;
				if pause {
					schedule.stop(Halt::Failures, now);
					report.stopped.push((schedule.id, Halt::Failures));
				}
				report.failed.push((schedule.id, reason));
			}
			Miss::Stop(halt, _) => {
				schedule.stop(halt, now);
				report.stopped.push((schedule.id, halt));
			}
		}
		schedule.updated_at = now;
		repository::save(&mut tx, tenant, &schedule).await?;
		if claim.is_some() {
			repository::insert_tick(
				&mut tx,
				tenant,
				schedule.workspace_id,
				&Tick {
					id: Uuid::new_v4(),
					schedule_id: schedule.id,
					scheduled_at,
					recorded_at: now,
					status,
					reason: Some(reason),
					task_id: None,
					thread_id: None,
					started_by: None,
				},
			)
			.await?;
		}
		Ok(())
	}
	.await;
	match result {
		Ok(()) => tx.commit().await,
		Err(error) => {
			tx.rollback().await?;
			Err(error)
		}
	}
}

/// Stops the Schedules of a person who left the Tenant, in the transaction
/// that disables their Mapping (#557 decision 3).
pub(crate) async fn stop_for_registrant(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
	subject: &str,
) -> Result<usize> {
	Ok(
		repository::stop(tx, tenant, Stop::Registrant(subject), Utc::now())
			.await?
			.len(),
	)
}

/// Stops the Schedules whose fixed Thread is deleted (#557 decision 21).
pub(crate) async fn stop_for_thread(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
	thread: Uuid,
) -> Result<usize> {
	Ok(
		repository::stop(tx, tenant, Stop::Thread(thread), Utc::now())
			.await?
			.len(),
	)
}

/// Stops every Schedule of a deleted Tenant; a restore leaves them stopped
/// (#557 decision 22).
pub(crate) async fn stop_for_tenant(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
) -> Result<usize> {
	Ok(repository::stop(tx, tenant, Stop::Tenant, Utc::now())
		.await?
		.len())
}

/// A Schedule tool call by an Agent (#557 decisions 5 to 7). It acts for the
/// person at the root of the Run's chain, with the credential the Run was
/// admitted with, in the Run's Workspace. Its approval was already decided by
/// that person's Approval Mode and Approval Rules before the call ran.
pub async fn agent_tool(
	store: &Store,
	run: &aidash_domain::Run,
	operation: &str,
	input: Value,
) -> Result<Value> {
	use aidash_application::ports::authorization::ExecutionGrantRepository;
	use aidash_application::schedules::tool;
	let federated = !run.is_local(store.deployment.as_str());
	let grant = if federated {
		None
	} else {
		crate::apps::identity::repositories::execution::Grants { store }
			.grant(run.id)
			.await?
	};
	let person = crate::apps::identity::repositories::oauth_connections::owner_of_run(store, run)
		.await?
		.map(|owner| owner.subject);
	let (tenant, credential) = match &grant {
		Some(grant) => (grant.tenant.clone(), grant.credential_id),
		None => (String::new(), Uuid::nil()),
	};
	let scheduled = match &grant {
		Some(_) => {
			let scope = aidash_application::tenancy::TenancyScope::tenant(&tenant)?;
			store
				.pool
				.scoped(&scope, async |tx| {
					repository::scheduled_chain(&mut **tx, &tenant, run.task_id).await
				})
				.await?
		}
		None => false,
	};
	let changes = operation != "schedule_list";
	let registrant = schedules::agent_registrant(
		schedules::Caller {
			root_person: person.as_deref(),
			federated,
			scheduled,
		},
		changes,
	)?
	.to_owned();
	let identity = SubjectIdentity {
		http_session: None,
		credential_id: credential,
		tenant: tenant.clone(),
		subject: registrant.clone(),
	};
	let mut access = Access::begin(store, &identity).await?;
	let result: Result<Value> = async {
		let thread_of_run = repository::task_thread(&mut *access.tx, run.task_id).await?;
		let this_thread = |choice: tool::Thread| -> Result<ThreadMode> {
			Ok(match choice {
				tool::Thread::New => ThreadMode::New,
				tool::Thread::This => ThreadMode::Fixed {
					thread_id: thread_of_run.ok_or_else(|| {
						Error::Invalid("this Run works in no Thread to keep".into())
					})?,
				},
			})
		};
		let mut scope = Adapter {
			access: &mut access,
			store,
			now: Utc::now(),
		};
		let summary = |schedule: &Schedule| {
			json!({
				"schedule_id": schedule.id,
				"title": schedule.definition.title,
				"status": schedule.status(),
				"next_scheduled_at": schedule.next_scheduled_at,
				"trigger": schedule.definition.trigger,
				"agent": schedule.definition.agent,
				"acts_as_you": schedule.registrant == registrant,
			})
		};
		// An Agent changes only its person's own Schedules (#557 decision 7),
		// and only in its Run's Workspace, the one it lists and registers in.
		let own = |schedule: &Schedule| -> Result<()> {
			if schedule.registrant == registrant && schedule.workspace_id == run.workspace_id {
				Ok(())
			} else {
				Err(Error::Invalid(
					"an Agent may change only Schedules of this Workspace that act as the person who started this conversation".into(),
				))
			}
		};
		Ok(match operation {
			"schedule_create" => {
				let input: tool::Create =
					serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
				let definition = Definition {
					title: input.title,
					request: input.request,
					agent: input.agent.unwrap_or_else(|| EntityRef {
						id: run.agent_id.clone(),
						version: run.agent_version.clone(),
					}),
					trigger: input.trigger,
					thread: this_thread(input.thread)?,
					approval_mode: input.approval_mode,
					max_lateness_seconds: input
						.max_lateness_seconds
						.unwrap_or(schedules::DEFAULT_MAX_LATENESS_SECONDS),
				};
				let schedule = app::register(
					&mut scope,
					run.workspace_id,
					definition,
					Origin::Agent { run_id: run.id },
				)
				.await?;
				summary(&schedule)
			}
			"schedule_list" => json!({
				"schedules": app::list(&mut scope, Listing::Workspace(run.workspace_id))
					.await?
					.iter()
					.map(summary)
					.collect::<Vec<_>>()
			}),
			"schedule_update" => {
				let input: tool::Update =
					serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
				own(&scope.load(input.schedule_id).await?)?;
				let patch = Patch {
					title: input.title,
					request: input.request,
					agent: input.agent,
					trigger: input.trigger,
					thread: input.thread.map(this_thread).transpose()?,
					approval_mode: input.approval_mode,
					max_lateness_seconds: input.max_lateness_seconds,
				};
				summary(&app::update(&mut scope, input.schedule_id, patch).await?)
			}
			"schedule_delete" => {
				let input: tool::Delete =
					serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
				own(&scope.load(input.schedule_id).await?)?;
				match input.action {
					tool::Removal::Delete => {
						app::delete(&mut scope, input.schedule_id).await?;
						json!({"deleted": input.schedule_id})
					}
					tool::Removal::Pause => {
						summary(&app::pause(&mut scope, input.schedule_id).await?)
					}
				}
			}
			_ => return Err(Error::Invalid("unknown Schedule tool".into())),
		})
	}
	.await;
	access.finish(result).await
}
