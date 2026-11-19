//! Duties (#1011, ADR 0142; from the Schedules of #557, ADR 0134):
//! authorized operations for people and Agents, the pass every replica runs
//! for Duties whose time trigger is due, and the notices the bell shows.
use super::access::Lease;
use crate::apps::workspaces::repositories::duties::{self as repository, Filter, Stop};
use crate::apps::workspaces::serializers::duties::{DetailView, DutyNotice, DutyView, NoticesView};
use crate::{
	Error, Result,
	authorization::{
		access::Access,
		identity::{Actor, SubjectIdentity},
	},
	federation::Federation,
	store::Store,
};
use aidash_application::duties::{self as app, DutyScope, Listing, Origin};
use aidash_domain::{
	approval_rules::{ApprovalMode, MODE_RESOURCE},
	duties::{self, Claim, Definition, Duty, Halt, Patch, Tick, TickStatus},
	policy::SubjectKind,
	registry::{EntityRef, Search},
	tenancy::TenantName,
};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

/// The person acting in one Duty transaction.
pub(crate) struct Adapter<'a> {
	pub access: &'a mut Access,
	pub store: &'a Store,
	pub now: DateTime<Utc>,
}

fn tenant(access: &Access) -> String {
	access.identity.tenant.clone()
}

#[async_trait::async_trait]
impl DutyScope for Adapter<'_> {
	fn subject(&self) -> &str {
		&self.access.identity.subject
	}
	async fn requester_credential(&mut self) -> aidash_application::Result<Uuid> {
		let (tenant, subject) = (tenant(self.access), self.subject().to_owned());
		Ok(
			repository::mapping_credential(&mut *self.access.tx, &tenant, &subject)
				.await?
				.unwrap_or(self.access.identity.credential_id),
		)
	}
	async fn created_by(
		&mut self,
		run: Uuid,
		key: &str,
	) -> aidash_application::Result<Option<Duty>> {
		let tenant = tenant(self.access);
		Ok(repository::created_by(&mut *self.access.tx, &tenant, run, key).await?)
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
		Ok(self.access.decide(&resource, duties::ADMINISTER).await?)
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
	async fn is_person(&mut self, subject: &str) -> aidash_application::Result<bool> {
		Ok(self
			.access
			.snapshot
			.bundle
			.subjects
			.get(subject)
			.is_some_and(|subject| subject.enabled && subject.kind == SubjectKind::User))
	}
	async fn tier_cap(&mut self) -> aidash_application::Result<Option<u32>> {
		let tenant = tenant(self.access);
		Ok(self
			.store
			.schedule_tier
			.duties(&TenantName::new(&tenant)?)
			.await?)
	}
	async fn count(&mut self) -> aidash_application::Result<i64> {
		let tenant = tenant(self.access);
		Ok(repository::count(&mut *self.access.tx, &tenant).await?)
	}
	async fn insert(&mut self, duty: &Duty) -> aidash_application::Result<()> {
		let tenant = tenant(self.access);
		Ok(repository::insert(&mut *self.access.tx, &tenant, duty).await?)
	}
	async fn load_for_update(&mut self, id: Uuid) -> aidash_application::Result<Duty> {
		let tenant = tenant(self.access);
		repository::load(&mut *self.access.tx, &tenant, id, true)
			.await?
			.ok_or_else(|| aidash_application::Error::NotFound("duty not found".into()))
	}
	async fn load(&mut self, id: Uuid) -> aidash_application::Result<Duty> {
		let tenant = tenant(self.access);
		repository::load(&mut *self.access.tx, &tenant, id, false)
			.await?
			.ok_or_else(|| aidash_application::Error::NotFound("duty not found".into()))
	}
	async fn save(&mut self, duty: &Duty) -> aidash_application::Result<()> {
		let tenant = tenant(self.access);
		repository::save(&mut *self.access.tx, &tenant, duty).await?;
		Ok(())
	}
	async fn delete(&mut self, id: Uuid) -> aidash_application::Result<()> {
		let tenant = tenant(self.access);
		Ok(repository::delete(&mut *self.access.tx, &tenant, id).await?)
	}
	async fn list(&mut self, listing: Listing) -> aidash_application::Result<Vec<Duty>> {
		let (tenant, subject) = (tenant(self.access), self.subject().to_owned());
		let filter = match listing {
			Listing::Workspace(workspace) => Filter::Workspace(workspace),
			Listing::Mine => Filter::Mine(&subject),
			Listing::Tenant => Filter::Tenant,
		};
		Ok(repository::list(&mut *self.access.tx, &tenant, filter).await?)
	}
	async fn ticks(&mut self, id: Uuid, limit: u32) -> aidash_application::Result<Vec<Tick>> {
		let tenant = tenant(self.access);
		Ok(repository::ticks(&mut *self.access.tx, &tenant, id, limit).await?)
	}
}

/// What a person asks of the Duties API.
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
		let administers = scope.administers().await?;
		let view = |duty: Duty| DutyView::new(duty, &mine, administers);
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
			Operation::Notices => serde_json::to_value(notices(&mut scope).await?)?,
			Operation::RunNow(_) => unreachable!("handled above"),
		})
	}
	.await;
	access.finish(result).await
}

/// The Duties the bell tells this person about (#557 decision 25): their
/// own that stopped without anyone choosing it, those waiting for them as a
/// named successor, and, for holders of `duty.administer`, those waiting
/// for a successor. Also the Tasks their Duties started this week, whose
/// completion the bell shows without a toast (decision 26).
async fn notices(scope: &mut Adapter<'_>) -> Result<NoticesView> {
	let administers = scope.administers().await?;
	let subject = scope.subject().to_owned();
	let tenant = tenant(scope.access);
	let all = if administers {
		repository::list(&mut *scope.access.tx, &tenant, Filter::Tenant).await?
	} else {
		repository::list(&mut *scope.access.tx, &tenant, Filter::Mine(&subject)).await?
	};
	let stopped = all
		.iter()
		.filter(|duty| {
			duty.halt.is_some_and(|halt| {
				halt.notifies()
					&& (duty.requester == subject
						|| duty.successor.as_deref() == Some(subject.as_str())
						|| (administers && halt == Halt::RequesterLeft))
			})
		})
		.map(DutyNotice::of)
		.collect();
	let own: Vec<Uuid> = all
		.iter()
		.filter(|duty| duty.requester == subject)
		.map(|duty| duty.id)
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
fn tick_key(duty: Uuid, scheduled_at: DateTime<Utc>, part: &str) -> Uuid {
	let digest = crate::registry::digest(&json!(["duty_tick", duty, scheduled_at, part]));
	let mut bytes = [0_u8; 16];
	for (index, byte) in bytes.iter_mut().enumerate() {
		*byte = u8::from_str_radix(&digest[index * 2..index * 2 + 2], 16).unwrap_or_default();
	}
	uuid::Builder::from_random_bytes(bytes).into_uuid()
}

/// Starts one Task of `duty` as the lease's subject, in the Duty's own
/// Thread. Its first Task opens that Thread with the mission as its first
/// message and records it on the Duty (#1011). Returns the Task and the
/// Thread.
async fn start(
	f: &Federation,
	lease: &mut Lease,
	duty: &Duty,
	key: (DateTime<Utc>, &str),
) -> Result<(Uuid, Uuid)> {
	let workspace = duty.workspace_id;
	let definition = &duty.definition;
	let thread = match duty.thread_id {
		Some(thread_id) => thread_id,
		None => {
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
					content: format!("{}\n\n{}", definition.title, definition.mission),
					thread_id: None,
					idempotency_key: tick_key(duty.id, key.0, &format!("{}:message", key.1)),
					attachment_ids: vec![],
					file_versions: vec![],
				},
			)
			.await?;
			let thread = super::threads::create(&f.store, lease, workspace, message.message.id)
				.await?
				.id;
			let tenant = tenant(lease.access_mut());
			repository::open_thread(lease.tx(), &tenant, duty.id, thread).await?;
			thread
		}
	};
	// "Run now" is an ordinary Run of the person who clicked it, in their own
	// mode; only a Task the Duty started carries the Duty's. The Duty's mode
	// is its Thread's mode for its Requester, so a Duty set back to the
	// default clears it (#557 decision 4).
	let scheduled = key.1.is_empty();
	if scheduled {
		let mode = definition.approval_mode;
		// The Duty's Approval Mode is its Thread's mode for the person
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
			mode,
		)
		.await?;
	}
	let (_, task) = crate::apps::execution::capabilities::services::endpoints::start_in_thread(
		f,
		lease.access_mut(),
		(workspace, thread, definition.agent.id.clone()),
		crate::apps::execution::capabilities::serializers::contracts::Enqueue {
			idempotency_key: tick_key(duty.id, key.0, &format!("{}:run", key.1)),
			agent_version: definition.agent.version.clone(),
			title: definition.title.clone(),
			description: definition.mission.clone(),
			read_pages: vec![],
		},
	)
	.await?;
	let task = task.ok_or_else(|| Error::Conflict("this tick already started".into()))?;
	Ok((task, thread))
}

/// "Run now": an ordinary Run of the Duty as the person who clicked it,
/// recorded in its history (#557 decision 24).
async fn run_now(f: &Federation, identity: &SubjectIdentity, id: Uuid) -> Result<Value> {
	let access = Access::begin(&f.store, identity).await?;
	let mut lease = Lease::from_access(access);
	let now = Utc::now();
	let result: Result<Value> = async {
		let tenant = identity.tenant.clone();
		let duty = repository::load(lease.tx(), &tenant, id, false)
			.await?
			.ok_or_else(|| Error::NotFound("duty not found".into()))?;
		let resource = lease.access_mut().workspace(duty.workspace_id).await?;
		if !lease
			.access_mut()
			.decide(&resource, "workspace.read")
			.await?
		{
			return Err(Error::NotFound("duty not found".into()));
		}
		let (task, thread) = start(f, &mut lease, &duty, (now, &identity.subject)).await?;
		repository::insert_tick(
			lease.tx(),
			&tenant,
			duty.workspace_id,
			&Tick {
				id: Uuid::new_v4(),
				duty_id: duty.id,
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

impl Pass {
	/// Ticks this pass started, skipped, failed or stopped.
	fn handled(&self) -> usize {
		self.started.len() + self.skipped.len() + self.failed.len() + self.stopped.len()
	}
}

/// Due Duties handled per pass; the rest wait for the next one.
const BATCH: usize = 32;

/// One pass of the scheduler on this replica: settle the ticks whose Runs
/// ended, then handle the due Duties no other replica holds. Every replica
/// runs it; `SKIP LOCKED` and the tick key keep a tick to one Task.
pub async fn pass(f: &Federation, now: DateTime<Utc>) -> Result<Pass> {
	let mut report = Pass::default();
	for ended in repository::ended(&f.store.pool, 256).await? {
		match settle(f, &ended, now).await {
			Ok(true) => report.settled += 1,
			Ok(false) => {}
			Err(error) => {
				tracing::warn!(%error, tick = %ended.tick, "a Duty tick could not be settled");
			}
		}
	}
	// A tick that failed transiently stays due, oldest first. Paging past it
	// tries each due Duty at most once per pass, so a few that keep
	// failing cannot hold back the rest of the Deployment; only ticks that
	// did something count toward the batch.
	let mut handled = 0;
	let mut after = None;
	'pages: loop {
		let page = repository::due(&f.store.pool, now, after, BATCH as u64).await?;
		let last_page = page.len() < BATCH;
		for (tenant, id, fire_at) in page {
			after = Some((fire_at, id));
			let before = report.handled();
			if let Err(error) = fire(f, &tenant, id, now, &mut report).await {
				tracing::warn!(%error, duty = %id, "a due Duty could not be handled");
			}
			if report.handled() > before {
				handled += 1;
				if handled >= BATCH {
					break 'pages;
				}
			}
		}
		if last_page {
			break;
		}
	}
	if !report.started.is_empty() {
		f.notify.notify_waiters();
	}
	Ok(report)
}

/// Settles one ended tick and counts a failure toward the Duty's pause.
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
		let Some(mut duty) = repository::load(&mut tx, &ended.tenant, ended.duty, true).await?
		else {
			return Ok(true);
		};
		let (failures, pause) = duties::after_settled(
			duty.consecutive_failures,
			ended.status,
			duty.definition.pause_after_failures,
		);
		if failures == duty.consecutive_failures && !pause {
			return Ok(true);
		}
		duty.consecutive_failures = failures;
		if pause && duty.halt.is_none() {
			duty.stop(Halt::Failures, now);
		}
		duty.updated_at = now;
		repository::save(&mut tx, &ended.tenant, &duty).await?;
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
#[derive(Debug, Clone, PartialEq, Eq)]
enum Miss {
	/// Recorded as failed; it counts toward the pause.
	Fail(String),
	/// The Duty stops for this reason, and the tick is recorded with this
	/// status: failed when its start was refused, skipped when it never tried.
	Stop(Halt, String, TickStatus),
}

/// Whether a conflict names a refusal by its code, such as
/// `RUNTIME_UNAVAILABLE: <detail>`, rather than a race that
/// the same request can lose once and win next time.
fn refusal_code(message: &str) -> bool {
	let code = message.split(':').next().unwrap_or_default();
	code.len() >= 3
		&& code
			.bytes()
			.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

/// What a start that failed after its claim records. A definite refusal is a
/// failed tick with its reason, never retried silently (#557, carried from
/// #540). A transient error, such as a policy revision that changed under
/// the start, an unavailable peer or a database error, records nothing: the
/// claim rolls back and the next pass tries the same tick again.
fn classify(error: &Error) -> Option<Miss> {
	match error {
		Error::Forbidden
		| Error::NotFound(_)
		| Error::Invalid(_)
		| Error::Unprocessable(_)
		| Error::Refused(_)
		| Error::AuthorityRefused(_)
		| Error::RemoteExecutionRefused(_)
		| Error::ResumeBlocked(_)
		| Error::RunHidden(_) => Some(Miss::Fail(error.to_string())),
		Error::Conflict(message) if refusal_code(message) => Some(Miss::Fail(message.clone())),
		_ => None,
	}
}

/// What a claimed tick did in its Requester's transaction.
enum Fired {
	/// Another replica held it, or it was no longer due.
	Nothing,
	Skipped(String),
	Started(Uuid),
}

/// Handles one due Duty as its Requester: claim, then start, skip or stop.
async fn fire(
	f: &Federation,
	tenant: &str,
	id: Uuid,
	now: DateTime<Utc>,
	report: &mut Pass,
) -> Result<()> {
	// Read who it acts as, then act as them; the claim below rechecks the row.
	let scope = aidash_application::tenancy::TenancyScope::tenant(tenant)?;
	let Some(duty) = f
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
		credential_id: duty.credential_id,
		tenant: tenant.to_owned(),
		subject: duty.requester.clone(),
	};
	let access = match Access::begin(&f.store, &identity).await {
		Ok(access) => access,
		// The Requester left the Tenant or lost the credential it acts with.
		// A refused credential says why since #876; any reason stops it.
		Err(
			Error::Unauthorized
			| Error::CredentialUnauthorized(_)
			| Error::Forbidden
			| Error::CredentialForbidden(_),
		) => {
			return record_miss(
				f,
				tenant,
				(&duty, duty.revision),
				None,
				now,
				Miss::Stop(
					Halt::RequesterLeft,
					"the Requester lost access".into(),
					TickStatus::Failed,
				),
				report,
			)
			.await;
		}
		Err(error) => return Err(error),
	};
	let mut lease = Lease::from_access(access);
	// The claimed tick and the revision it was judged on.
	let mut claimed: Option<(Claim, i64)> = None;
	// Set when the tick shows the Duty must stop, before the start rolls back.
	let mut stopping: Option<(Halt, String, TickStatus)> = None;
	let result: Result<Fired> = async {
		let Some(duty) = repository::claim(lease.tx(), tenant, id, now).await? else {
			return Ok(Fired::Nothing);
		};
		let Some(due) = duty.next_scheduled_at else {
			return Ok(Fired::Nothing);
		};
		let Some(claim) = duty.timing().claim(due, now)? else {
			return Ok(Fired::Nothing);
		};
		claimed = Some((claim.clone(), duty.revision));
		// While the previous Task is open the tick is skipped: the default
		// a Router may later replace (#1011).
		let skip = if claim.late {
			Some(duties::skip::LATE.to_owned())
		} else if repository::running(lease.tx(), tenant, id).await? {
			Some(duties::skip::OVERLAP.to_owned())
		} else {
			None
		};
		if let Some(reason) = skip {
			advance(lease.tx(), tenant, &duty, &claim, now).await?;
			skipped_tick(lease.tx(), tenant, &duty, &claim, &reason, now).await?;
			return Ok(Fired::Skipped(reason));
		}
		// The Requester must still be able to register it and start Tasks
		// in its Workspace; otherwise they lost access there.
		let resource = lease.access_mut().workspace(duty.workspace_id).await?;
		for action in ["workspace.read", "task.create", duties::CREATE] {
			if !lease.access_mut().decide(&resource, action).await? {
				stopping = Some((
					Halt::RequesterLeft,
					format!("the Requester may no longer {action} in this Workspace"),
					TickStatus::Failed,
				));
				return Err(Error::Forbidden);
			}
		}
		// Only a refusal stops the Duty; any other error, such as a
		// database outage, leaves the tick to the next pass.
		if let Some(thread_id) = duty.thread_id {
			match crate::apps::execution::capabilities::services::endpoints::readable_thread(
				lease.access_mut(),
				duty.workspace_id,
				thread_id,
			)
			.await
			{
				Ok(()) => {}
				Err(error @ (Error::NotFound(_) | Error::Forbidden)) => {
					stopping = Some((
						Halt::ThreadDeleted,
						"the Duty's Thread was deleted".into(),
						TickStatus::Failed,
					));
					return Err(error);
				}
				Err(error) => return Err(error),
			}
		}
		match crate::apps::identity::services::catalog::entry(
			lease.access_mut(),
			&duty.definition.agent,
			"agent.execute",
		)
		.await
		{
			Ok(_) => {}
			Err(error @ (Error::NotFound(_) | Error::Forbidden)) => {
				let reason = format!(
					"Agent {}@{} can no longer be started",
					duty.definition.agent.id, duty.definition.agent.version
				);
				stopping = Some((Halt::AgentVersionUnusable, reason, TickStatus::Failed));
				return Err(error);
			}
			Err(error) => return Err(error),
		}
		// The usage gate judges the tick as it would a Run started by hand; its
		// refusal is a skip (limit), and any later refusal a failed start.
		if let aidash_application::usage::Verdict::Refuse(refusal) = f
			.store
			.admission
			.admit(tenant, aidash_application::usage::Work::Run)
			.await?
		{
			let reason = format!("{}: {}", duties::skip::LIMIT, refusal.code);
			advance(lease.tx(), tenant, &duty, &claim, now).await?;
			skipped_tick(lease.tx(), tenant, &duty, &claim, &reason, now).await?;
			return Ok(Fired::Skipped(reason));
		}
		let (task, thread) = start(f, &mut lease, &duty, (claim.scheduled_at, "")).await?;
		// Saving the Duty past this tick keeps the Thread its first Task opened.
		let duty = Duty {
			thread_id: Some(thread),
			..duty
		};
		advance(lease.tx(), tenant, &duty, &claim, now).await?;
		repository::insert_tick(
			lease.tx(),
			tenant,
			duty.workspace_id,
			&Tick {
				id: Uuid::new_v4(),
				duty_id: duty.id,
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
				Fired::Skipped(reason) => report.skipped.push((id, reason)),
				Fired::Nothing => {}
			}
			return Ok(());
		}
		Err(error) => lease.finish::<()>(Err(error)).await.unwrap_err(),
	};
	let Some((claim, revision)) = claimed else {
		return Err(outcome);
	};
	let miss = match stopping {
		Some((halt, reason, status)) => Miss::Stop(halt, reason, status),
		None => match classify(&outcome) {
			Some(miss) => miss,
			// Transient: nothing is recorded and the next pass retries.
			None => return Err(outcome),
		},
	};
	record_miss(
		f,
		tenant,
		(&duty, revision),
		Some(&claim),
		now,
		miss,
		report,
	)
	.await
}

/// Moves a claimed Duty past the tick it handled.
async fn advance(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	duty: &Duty,
	claim: &Claim,
	now: DateTime<Utc>,
) -> Result<()> {
	let mut next = duty.clone();
	next.next_scheduled_at = claim.next;
	next.updated_at = now;
	repository::save(&mut *tx, tenant, &next).await?;
	Ok(())
}

async fn skipped_tick(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	duty: &Duty,
	claim: &Claim,
	reason: &str,
	now: DateTime<Utc>,
) -> Result<()> {
	repository::insert_tick(
		&mut *tx,
		tenant,
		duty.workspace_id,
		&Tick {
			id: Uuid::new_v4(),
			duty_id: duty.id,
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
	(read, revision): (&Duty, i64),
	claim: Option<&Claim>,
	now: DateTime<Utc>,
	miss: Miss,
	report: &mut Pass,
) -> Result<()> {
	let scope = aidash_application::tenancy::TenancyScope::tenant(tenant)?;
	let mut tx = f.store.pool.begin(&scope).await?;
	let result: Result<()> = async {
		let Some(mut duty) = repository::load(&mut tx, tenant, read.id, true).await? else {
			return Ok(());
		};
		// Another replica handled this tick meanwhile, or a person changed the
		// Duty after the failed start was judged, such as moving it to a
		// usable Agent version: the outcome no longer applies, and the next
		// pass judges the Duty as it is now.
		if duty.halt.is_some() || duty.revision != revision {
			return Ok(());
		}
		let scheduled_at = claim.map_or(now, |claim| claim.scheduled_at);
		let (status, reason) = match &miss {
			Miss::Fail(reason) => (TickStatus::Failed, reason.clone()),
			Miss::Stop(_, reason, status) => (*status, reason.clone()),
		};
		if let Some(claim) = claim {
			duty.next_scheduled_at = claim.next;
		}
		match miss {
			Miss::Fail(reason) => {
				let (failures, pause) = duties::after_settled(
					duty.consecutive_failures,
					TickStatus::Failed,
					duty.definition.pause_after_failures,
				);
				duty.consecutive_failures = failures;
				if pause {
					duty.stop(Halt::Failures, now);
					report.stopped.push((duty.id, Halt::Failures));
				}
				report.failed.push((duty.id, reason));
			}
			Miss::Stop(halt, reason, _) => {
				duty.stop_because(halt, Some(reason), now);
				report.stopped.push((duty.id, halt));
			}
		}
		duty.updated_at = now;
		repository::save(&mut tx, tenant, &duty).await?;
		if claim.is_some() {
			repository::insert_tick(
				&mut tx,
				tenant,
				duty.workspace_id,
				&Tick {
					id: Uuid::new_v4(),
					duty_id: duty.id,
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

/// Stops the Duties of a person who left the Tenant, in the transaction
/// that disables their Mapping (#557 decision 3).
pub(crate) async fn stop_for_requester(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
	subject: &str,
) -> Result<usize> {
	Ok(
		repository::stop(tx, tenant, Stop::Requester(subject), Utc::now())
			.await?
			.len(),
	)
}

/// Stops the Duties whose fixed Thread is deleted (#557 decision 21).
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

/// Stops every Duty of a deleted Tenant; a restore leaves them stopped
/// (#557 decision 22).
pub(crate) async fn stop_for_tenant(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
) -> Result<usize> {
	Ok(repository::stop(tx, tenant, Stop::Tenant, Utc::now())
		.await?
		.len())
}

/// A Duty tool call by an Agent (#557 decisions 5 to 7, #1011). It acts for
/// the person at the root of the Run's chain, with the credential the Run
/// was admitted with, in the Run's Workspace. A Run a Duty started may set
/// up Duties too (#1012). Its approval was already decided by that person's
/// Approval Mode and Approval Rules before the call ran.
pub async fn agent_tool(
	store: &Store,
	run: &aidash_domain::Run,
	key: &str,
	operation: &str,
	input: Value,
) -> Result<Value> {
	use aidash_application::duties::tool;
	use aidash_application::ports::authorization::ExecutionGrantRepository;
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
	let requester = duties::agent_requester(duties::Caller {
		root_person: person.as_deref(),
		federated,
	})?
	.to_owned();
	let identity = SubjectIdentity {
		http_session: None,
		credential_id: credential,
		tenant: tenant.clone(),
		subject: requester.clone(),
	};
	let mut access = Access::begin(store, &identity).await?;
	let result: Result<Value> = async {
		let mut scope = Adapter {
			access: &mut access,
			store,
			now: Utc::now(),
		};
		let summary = |duty: &Duty| {
			json!({
				"duty_id": duty.id,
				"title": duty.definition.title,
				"status": duty.status(),
				"next_scheduled_at": duty.next_scheduled_at,
				"trigger": duty.definition.trigger,
				"thread_id": duty.thread_id,
				"agent": duty.definition.agent,
				"acts_as_you": duty.requester == requester,
			})
		};
		// An Agent changes only its person's own Duties (#557 decision 7),
		// and only in its Run's Workspace, the one it lists and registers in.
		let own = |duty: &Duty| -> Result<()> {
			if duty.requester == requester && duty.workspace_id == run.workspace_id {
				Ok(())
			} else {
				Err(Error::Invalid(
					"an Agent may change only Duties of this Workspace that act as the person who started this conversation".into(),
				))
			}
		};
		Ok(match operation {
			"duty_create" => {
				let input: tool::Create =
					serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
				let definition = Definition {
					title: input.title,
					mission: input.mission,
					procedure: input.procedure.unwrap_or_default(),
					agent: input.agent.unwrap_or_else(|| EntityRef {
						id: run.agent_id.clone(),
						version: run.agent_version.clone(),
					}),
					trigger: input.trigger,
					approval_mode: input.approval_mode,
					max_lateness_seconds: input
						.max_lateness_seconds
						.unwrap_or(duties::DEFAULT_MAX_LATENESS_SECONDS),
					pause_after_failures: input
						.pause_after_failures
						.unwrap_or(duties::DEFAULT_PAUSE_AFTER_FAILURES),
				};
				let duty = app::register(
					&mut scope,
					run.workspace_id,
					definition,
					Origin::Agent {
						run_id: run.id,
						key: key.to_owned(),
					},
				)
				.await?;
				summary(&duty)
			}
			"duty_list" => json!({
				"duties": app::list(&mut scope, Listing::Workspace(run.workspace_id))
					.await?
					.iter()
					.map(summary)
					.collect::<Vec<_>>()
			}),
			"duty_update" => {
				let input: tool::Update =
					serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
				own(&scope.load(input.duty_id).await?)?;
				let patch = Patch {
					title: input.title,
					mission: input.mission,
					procedure: input.procedure,
					agent: input.agent,
					trigger: input.trigger,
					approval_mode: input.approval_mode,
					max_lateness_seconds: input.max_lateness_seconds,
					pause_after_failures: input.pause_after_failures,
				};
				summary(&app::update(&mut scope, input.duty_id, patch).await?)
			}
			"duty_delete" => {
				let input: tool::Delete =
					serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
				own(&scope.load(input.duty_id).await?)?;
				match input.action {
					tool::Removal::Delete => {
						app::delete(&mut scope, input.duty_id).await?;
						json!({"deleted": input.duty_id})
					}
					tool::Removal::Pause => {
						summary(&app::pause(&mut scope, input.duty_id).await?)
					}
				}
			}
			_ => return Err(Error::Invalid("unknown Duty tool".into())),
		})
	}
	.await;
	access.finish(result).await
}

#[cfg(test)]
mod tests {
	use super::*;
	use rstest::rstest;

	#[rstest]
	#[case(Error::Conflict("RUNTIME_UNAVAILABLE: no isolated runner configured".into()), true)]
	#[case(Error::Forbidden, true)]
	#[case(Error::NotFound("agent".into()), true)]
	#[case(Error::Refused("Subnode has ended".into()), true)]
	#[case(Error::Conflict("authorization revision changed".into()), false)]
	#[case(Error::PolicyBusy { retry_after: 1 }, false)]
	#[case(Error::RateLimited, false)]
	#[case(Error::External("peer unavailable".into()), false)]
	fn only_a_definite_refusal_burns_a_tick(#[case] error: Error, #[case] recorded: bool) {
		// Act
		let miss = classify(&error);
		// Assert: a transient error records nothing, so the next pass retries.
		assert_eq!(miss.is_some(), recorded, "{error:?}");
		if let Some(Miss::Fail(reason)) = miss {
			assert_eq!(reason, error.to_string());
		}
	}
}
