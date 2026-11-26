//! Duties (#1011, ADR 0142; from the Schedules of #557, ADR 0134):
//! authorized operations for people and Agents, the pass every replica runs
//! for Duties whose time trigger is due, and the notices the bell shows.
use super::access::Lease;
use crate::apps::workspaces::repositories::{
	duties::{self as repository, Filter, Stop},
	scratch::{self, NativeScratchServices, ScratchAdapter},
};
use crate::apps::workspaces::serializers::duties::{DetailView, DutyNotice, DutyView, NoticesView};
use crate::{
	Error, Result,
	apps::execution::repositories::store::DutyTaskStart,
	authorization::{
		access::Access,
		identity::{Actor, SubjectIdentity},
	},
	federation::Federation,
	store::Store,
};
use aidash_application::{
	Error as AppError,
	duties::{self as app, DutyScope, Listing, Origin},
};
use aidash_domain::{
	approval_rules::{ApprovalMode, MODE_RESOURCE},
	duties::{self, Claim, Definition, Duty, Halt, Patch, Tick, TickStatus},
	policy::SubjectKind,
	registry::{EntityRef, Search},
	scratch::ScratchState,
	tenancy::TenantName,
};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

/// The person acting in one Duty transaction.
pub(crate) struct Adapter<'a> {
	pub access: &'a mut Access,
	pub store: &'a Store,
	pub scratch_policy: &'a crate::config::ScratchOperatorPolicy,
	pub now: DateTime<Utc>,
}

fn tenant(access: &Access) -> String {
	access.identity.tenant.clone()
}

/// Where a Thread is, deleted or not: a deleted Thread keeps its row.
struct ThreadPlace {
	node: String,
	scratch_space_id: Uuid,
	deleted_at: Option<DateTime<Utc>>,
}

impl Adapter<'_> {
	async fn thread_place(&mut self, thread: Uuid) -> Result<ThreadPlace> {
		use reinhardt::query::QueryStatementBuilder as _;
		use reinhardt::query::{Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query};
		let tenant = tenant(self.access);
		let (node, scratch_space_id, deleted_at): (String, Uuid, Option<DateTime<Utc>>) =
			crate::database::native::query_as(
				&Query::select()
					.column(Alias::new("node"))
					.column(Alias::new("scratch_space_id"))
					.column(Alias::new("deleted_at"))
					.from(Alias::new("threads"))
					.and_where(Expr::col("tenant").eq(tenant))
					.and_where(Expr::col("id").eq(Expr::value(thread)))
					.to_string(PostgresQueryBuilder),
			)
			.columns(&["node", "scratch_space_id", "deleted_at"])
			.fetch_optional(&mut **self.access.tx)
			.await?
			.ok_or_else(|| Error::NotFound("thread".into()))?;
		Ok(ThreadPlace {
			node,
			scratch_space_id,
			deleted_at,
		})
	}

	/// Opens a Duty's own Thread at `place`, created by the Requester, with
	/// the Duty's title and its mission as the first message, as a person's
	/// request is in a Thread a person opens, followed by `note`.
	async fn open_in(
		&mut self,
		id: Uuid,
		place: &ThreadPlace,
		definition: &Definition,
		note: Option<&str>,
	) -> Result<Uuid> {
		let tenant = tenant(self.access);
		let node = aidash_domain::nodes::NodeName::parse(&place.node)?;
		let requester = self.subject().to_owned();
		let thread = self
			.store
			.create_thread_in(
				&mut self.access.tx,
				crate::apps::execution::repositories::store::ThreadInsert {
					id,
					tenant: &tenant,
					node: &node,
					title: definition.title.trim(),
					scratch_space_id: place.scratch_space_id,
					created_by: &requester,
				},
			)
			.await?;
		self.store
			.record_thread_tenant_in(&mut self.access.tx, thread.id, &tenant, &requester)
			.await?;
		let mut first = format!("{}\n\n{}", definition.title.trim(), definition.mission);
		if let Some(note) = note {
			first.push_str("\n\n");
			first.push_str(note);
		}
		crate::apps::workspaces::models::Message::append_in(
			&mut *self.access.tx,
			thread.id,
			&requester,
			&first,
			&format!("duty-mission:{}", thread.id),
			None,
		)
		.await?;
		Ok(thread.id)
	}
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
	async fn require_thread(
		&mut self,
		thread: Uuid,
		action: &str,
	) -> aidash_application::Result<()> {
		let resource = self.access.thread(thread).await?;
		self.access.require(&resource, action).await?;
		Ok(())
	}
	async fn sees_thread(&mut self, thread: Uuid) -> aidash_application::Result<bool> {
		let resource = match self.access.thread(thread).await {
			Ok(resource) => resource,
			Err(Error::NotFound(_) | Error::Forbidden) => return Ok(false),
			Err(error) => return Err(error.into()),
		};
		Ok(self.access.decide(&resource, "thread.read").await?)
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
	async fn require_agent(
		&mut self,
		agent: &EntityRef,
		selection: Option<&aidash_domain::TaskModel>,
	) -> aidash_application::Result<()> {
		let entry =
			crate::apps::identity::services::catalog::entry(self.access, agent, "agent.execute")
				.await?;
		let caller = self.access.subjects.clone();
		crate::apps::registry::repositories::bindings::check_task_model(
			self.store,
			false,
			self.access,
			&entry,
			selection,
			&caller,
		)
		.await?;
		use reinhardt::query::QueryStatementBuilder as _;
		// A generated agent staffs only the Duties of the person whose chain
		// generated it, while it is live (#1012 decision 49); anyone else
		// needs a person's Catalog Approval of an agent of their own.
		let (tenant, subject) = (tenant(self.access), self.subject().to_owned());
		let refused: bool = crate::database::native::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::CustomWithExpr(
					"(EXISTS (SELECT 1 FROM generation_requests WHERE tenant = ? AND agent_id = ? AND agent_version = ?) AND NOT EXISTS (SELECT 1 FROM generation_requests WHERE tenant = ? AND agent_id = ? AND agent_version = ? AND root_subject = ? AND status = 'ACTIVE' AND expires_at > CLOCK_TIMESTAMP() AND foreign_intent IS NULL))".to_owned(),
					vec![
						reinhardt::query::Expr::value(tenant.clone()).into(),
						reinhardt::query::Expr::value(agent.id.clone()).into(),
						reinhardt::query::Expr::value(agent.version.clone()).into(),
						reinhardt::query::Expr::value(tenant).into(),
						reinhardt::query::Expr::value(agent.id.clone()).into(),
						reinhardt::query::Expr::value(agent.version.clone()).into(),
						reinhardt::query::Expr::value(subject).into(),
					],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.scalar_one(&mut **self.access.tx)
		.await?;
		if refused {
			return Err(aidash_application::Error::Invalid(
				"a generated agent works only for the person whose request generated it, while it is live".into(),
			));
		}
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
	async fn open_thread(
		&mut self,
		from: Uuid,
		definition: &Definition,
	) -> aidash_application::Result<Uuid> {
		// The Requester opens the Duty's own Thread in the Node and
		// Scratch Space it was registered from (#1008, ADR 0140).
		let place = self.thread_place(from).await?;
		Ok(self
			.open_in(Uuid::new_v4(), &place, definition, None)
			.await?)
	}
	async fn thread_deleted_at(
		&mut self,
		thread: Uuid,
	) -> aidash_application::Result<Option<DateTime<Utc>>> {
		Ok(self.thread_place(thread).await?.deleted_at)
	}
	async fn reopen_thread(
		&mut self,
		deleted: Uuid,
		deleted_at: DateTime<Utc>,
		definition: &Definition,
	) -> aidash_application::Result<Uuid> {
		// Only where the deleted Thread was: a Duty never moves to another
		// Scratch Space unseen (#991, ADR 0145).
		let place = self.thread_place(deleted).await?;
		let tenant = tenant(self.access);
		let person = self.subject().to_owned();
		let node = aidash_domain::nodes::NodeName::parse(&place.node)?;
		let mut services = NativeScratchServices {
			policy: self.scratch_policy,
		};
		let mut scope = ScratchAdapter {
			access: self.access,
			services: &mut services,
			person_override: None,
		};
		// The selection holds the Scratch Space lock through the Thread and
		// Duty writes in this same transaction.
		let selection = aidash_application::scratch::select_for_thread(
			&mut scope,
			&node,
			&definition.title,
			Some(place.scratch_space_id),
			self.now,
		)
		.await;
		if let Err(error) = selection {
			if !matches!(error, AppError::Forbidden | AppError::NotFound(_)) {
				return Err(error);
			}
			// Explain why the original Space cannot be selected. The selection
			// already locked it if it exists.
			let reason =
				match scratch::read(&mut *self.access.tx, &tenant, place.scratch_space_id).await? {
					None => "the Scratch Space of the Duty's deleted Thread no longer exists",
					Some(space) if space.state != ScratchState::Active => {
						"the Scratch Space of the Duty's deleted Thread can no longer be used"
					}
					Some(space) if !space.may_use(&person) => {
						"the Duty's new Thread opens in the Scratch Space of its deleted Thread, which \
					 you cannot use; ask its creator to add you as a member"
					}
					Some(_) if matches!(error, AppError::Forbidden) => {
						"the Scratch Space of the Duty's deleted Thread cannot be used with the current \
					 Node access or execution policy"
					}
					Some(_) => return Err(error),
				};
			return Err(AppError::Invalid(reason.into()));
		}
		let note = format!(
			"The Duty's previous Thread was deleted at {}.",
			deleted_at.format("%Y-%m-%d %H:%M UTC")
		);
		let id = Uuid::new_v4();
		let resource =
			self.access
				.resource("thread", id, json!({"owner": person, "thread_id": id}));
		self.access.require(&resource, "thread.create").await?;
		Ok(self.open_in(id, &place, definition, Some(&note)).await?)
	}
	async fn bind_replacement_thread(
		&mut self,
		duty: &Duty,
		deleted: Uuid,
	) -> aidash_application::Result<()> {
		let tenant = tenant(self.access);
		repository::bind_replacement_thread(&mut *self.access.tx, &tenant, duty, deleted).await?;
		self.access.tx.forget_thread_owner(deleted);
		self.access.tx.forget_thread_owner(duty.thread_id);
		Ok(())
	}
	async fn insert(&mut self, duty: &Duty) -> aidash_application::Result<()> {
		let tenant = tenant(self.access);
		repository::insert(&mut *self.access.tx, &tenant, duty).await?;
		// Every script version is kept with its author and time (#1011).
		let author = self.subject().to_owned();
		crate::apps::workspaces::repositories::duty_scripts::save_versions(
			&mut *self.access.tx,
			&tenant,
			None,
			duty,
			(&author, self.now),
		)
		.await?;
		Ok(())
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
		let before = repository::load(&mut *self.access.tx, &tenant, duty.id, false).await?;
		repository::save(&mut *self.access.tx, &tenant, duty).await?;
		let author = self.subject().to_owned();
		crate::apps::workspaces::repositories::duty_scripts::save_versions(
			&mut *self.access.tx,
			&tenant,
			before.as_ref().map(|before| &before.definition),
			duty,
			(&author, self.now),
		)
		.await?;
		Ok(())
	}
	async fn delete(&mut self, id: Uuid) -> aidash_application::Result<()> {
		let tenant = tenant(self.access);
		Ok(repository::delete(&mut *self.access.tx, &tenant, id).await?)
	}
	async fn hand_over(&mut self, duty: &Duty, successor: &str) -> aidash_application::Result<()> {
		let tenant = tenant(self.access);
		// Only a Thread the Duty opened is its own to hand over; a Thread it
		// was registered in stays its owner's. A deleted one is replaced on
		// the successor's first edit or resume instead (ADR 0145).
		if duty.origin_thread_id.is_some()
			&& self
				.thread_place(duty.thread_id)
				.await?
				.deleted_at
				.is_none()
		{
			repository::take_over_thread(&mut *self.access.tx, &tenant, duty.thread_id, successor)
				.await?;
			self.access.tx.forget_thread_owner(duty.thread_id);
		}
		crate::apps::workspaces::repositories::duty_events::drop_bookings(
			&mut *self.access.tx,
			&tenant,
			duty.id,
		)
		.await?;
		Ok(())
	}
	async fn budget_setters(&mut self, duty: Uuid) -> aidash_application::Result<Vec<String>> {
		let tenant = tenant(self.access);
		Ok(repository::budget_setters(&mut *self.access.tx, &tenant, duty).await?)
	}
	async fn manages_budgets(&mut self) -> aidash_application::Result<bool> {
		let resource = self
			.access
			.resource("tenant", tenant(self.access), json!({}));
		Ok(self
			.access
			.decide(
				&resource,
				aidash_domain::identity::tenant_administration::BUDGET_MANAGE,
			)
			.await?)
	}
	async fn list(&mut self, listing: Listing) -> aidash_application::Result<Vec<Duty>> {
		let (tenant, subject) = (tenant(self.access), self.subject().to_owned());
		let filter = match listing {
			Listing::Thread(thread) => Filter::Thread(thread),
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

/// Script versions a Duty's page shows.
const SCRIPT_VERSIONS: u32 = 20;

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
	TakeOver(Uuid),
	RunNow(Uuid),
	Notices,
}

/// Duties as the viewer sees them, each saying whether its own Thread was
/// deleted (ADR 0145).
async fn views(
	scope: &mut Adapter<'_>,
	duties: Vec<Duty>,
	viewer: &str,
	administers: bool,
) -> Result<Vec<DutyView>> {
	let tenant = tenant(scope.access);
	let threads: Vec<Uuid> = duties.iter().map(|duty| duty.thread_id).collect();
	let deleted = repository::deleted_threads(&mut *scope.access.tx, &tenant, &threads).await?;
	Ok(duties
		.into_iter()
		.map(|duty| DutyView::new(duty, viewer, administers).with_deleted(&deleted))
		.collect())
}

pub(crate) async fn view(
	scope: &mut Adapter<'_>,
	duty: Duty,
	viewer: &str,
	administers: bool,
) -> Result<DutyView> {
	let tenant = tenant(scope.access);
	let deleted =
		repository::deleted_threads(&mut *scope.access.tx, &tenant, &[duty.thread_id]).await?;
	Ok(DutyView::new(duty, viewer, administers).with_deleted(&deleted))
}

async fn one(
	scope: &mut Adapter<'_>,
	duty: Duty,
	viewer: &str,
	administers: bool,
) -> Result<Value> {
	Ok(serde_json::to_value(
		view(scope, duty, viewer, administers).await?,
	)?)
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
			scratch_policy: &f.config.scratch_policy,
			now,
		};
		let mine = scope.subject().to_owned();
		let administers = scope.administers().await?;
		Ok(match op {
			Operation::Create(thread, definition) => {
				let duty = app::register(&mut scope, thread, definition, Origin::Person).await?;
				one(&mut scope, duty, &mine, administers).await?
			}
			Operation::List(listing) => {
				let duties = app::list(&mut scope, listing).await?;
				serde_json::to_value(views(&mut scope, duties, &mine, administers).await?)?
			}
			Operation::Read(id) => {
				let detail = app::detail(&mut scope, id).await?;
				let tenant = tenant(scope.access);
				let deleted = repository::deleted_threads(
					&mut *scope.access.tx,
					&tenant,
					&[detail.duty.thread_id],
				)
				.await?;
				let versions = crate::apps::workspaces::repositories::duty_scripts::versions(
					&mut *scope.access.tx,
					&tenant,
					detail.duty.id,
					SCRIPT_VERSIONS,
				)
				.await?;
				// A later version of its template is offered, never applied
				// (#1011 D4).
				let template_update = match &detail.duty.template {
					Some(source) => {
						crate::apps::marketplace::services::duty_templates::newer(
							scope.access,
							f.store.deployment.as_str(),
							source,
						)
						.await?
					}
					None => None,
				};
				let mut shown = DetailView::new(detail, &mine, versions, template_update);
				shown.duty = shown.duty.with_deleted(&deleted);
				serde_json::to_value(shown)?
			}
			Operation::Update(id, patch) => {
				let duty = app::update(&mut scope, id, patch).await?;
				one(&mut scope, duty, &mine, administers).await?
			}
			Operation::Pause(id) => {
				let duty = app::pause(&mut scope, id).await?;
				one(&mut scope, duty, &mine, administers).await?
			}
			Operation::Resume(id) => {
				let duty = app::resume(&mut scope, id).await?;
				one(&mut scope, duty, &mine, administers).await?
			}
			Operation::Delete(id) => {
				app::delete(&mut scope, id).await?;
				json!({"deleted": id})
			}
			Operation::Successor(id, successor) => {
				let duty = app::name_successor(&mut scope, id, &successor).await?;
				one(&mut scope, duty, &mine, administers).await?
			}
			Operation::TakeOver(id) => {
				let duty = app::take_over(&mut scope, id).await?;
				one(&mut scope, duty, &mine, administers).await?
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

/// Starts a Task of the Duty, as its Requester, in the Duty's own Thread
/// (#1008, ADR 0140). Returns the Task.
async fn start(
	f: &Federation,
	lease: &mut Lease,
	duty: &Duty,
	key: (DateTime<Utc>, &str),
) -> Result<Uuid> {
	let description = duty.definition.mission.clone();
	let title = duty.definition.title.clone();
	start_with(f, lease, duty, key, (title, description)).await
}

/// [`start`] with the Task title and description an event, a Router or a
/// script writes (#1011).
pub(crate) async fn start_with(
	f: &Federation,
	lease: &mut Lease,
	duty: &Duty,
	key: (DateTime<Utc>, &str),
	(title, description): (String, String),
) -> Result<Uuid> {
	let thread = duty.thread_id;
	let definition = &duty.definition;
	// "Run now" is an ordinary Run of the person who clicked it, in their own
	// mode; only a Task the Duty started carries the Duty's. The Duty's mode
	// is its Thread's mode for its Requester, so a Duty set back to the
	// default clears it (#557 decision 4).
	let scheduled = key.1.is_empty();
	if scheduled {
		let mode = definition.approval_mode;
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
		(thread, definition.agent.id.clone()),
		crate::apps::execution::capabilities::serializers::contracts::Enqueue {
			idempotency_key: tick_key(duty.id, key.0, &format!("{}:run", key.1)),
			task_model: definition.task_model.clone(),
			agent_version: definition.agent.version.clone(),
			title,
			description,
			read_pages: vec![],
		},
		Some(if scheduled {
			DutyTaskStart::Automatic(duty.id)
		} else {
			DutyTaskStart::Manual(duty.id)
		}),
	)
	.await?;
	task.ok_or_else(|| Error::Conflict("this tick already started".into()))
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
		let resource = lease.access_mut().thread(duty.thread_id).await?;
		if !lease.access_mut().decide(&resource, "thread.read").await? {
			return Err(Error::NotFound("duty not found".into()));
		}
		let task = start(f, &mut lease, &duty, (now, &identity.subject)).await?;
		repository::insert_tick(
			lease.tx(),
			&tenant,
			duty.thread_id,
			&Tick {
				id: Uuid::new_v4(),
				duty_id: duty.id,
				scheduled_at: now,
				recorded_at: now,
				status: TickStatus::Started,
				reason: None,
				task_id: Some(task),
				started_by: Some(identity.subject.clone()),
			},
		)
		.await?;
		Ok(json!({"task_id": task, "thread_id": duty.thread_id}))
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
	/// Router and Watcher Tasks whose outcome the Duty applied (#1011).
	pub scripts_settled: usize,
	/// Watcher Tasks this pass started.
	pub watchers_started: Vec<Uuid>,
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
	// What Routers decided and Watchers found (#1011).
	report.scripts_settled = super::duty_scripts::settle_all(f, now).await;
	if super::duty_events::deliver_pending(f).await? {
		f.notify.notify_waiters();
	}
	forget(f, now).await;
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
	// Watchers run on their own interval, apart from the time trigger.
	report.watchers_started = super::duty_scripts::run_watchers(f, now).await;
	if !report.started.is_empty()
		|| !report.watchers_started.is_empty()
		|| report.scripts_settled > 0
	{
		f.notify.notify_waiters();
	}
	Ok(report)
}

/// Rows a Duty keeps only for a while (#1011): webhook deliveries past their
/// retention, and bookings whose Run ended, which no event can reach.
async fn forget(f: &Federation, now: DateTime<Utc>) {
	use crate::apps::workspaces::repositories::duty_events;
	/// Rows of each kind forgotten per pass; the rest wait for the next.
	const FORGET: u64 = 1000;
	let before = now - Duration::days(duty_events::DELIVERY_RETENTION_DAYS);
	if let Err(error) = duty_events::forget_deliveries(&f.store.pool, before, FORGET).await {
		tracing::warn!(%error, "old Duty webhook deliveries could not be forgotten");
	}
	if let Err(error) = duty_events::forget_ended_bookings(&f.store.pool, FORGET).await {
		tracing::warn!(%error, "bookings of ended Runs could not be dropped");
	}
}

/// Settles one ended tick and counts a failure toward the Duty's pause.
async fn settle(f: &Federation, ended: &repository::Ended, now: DateTime<Utc>) -> Result<bool> {
	let scope = aidash_application::tenancy::TenancyScope::tenant(&ended.tenant)?;
	let mut tx = f.store.pool.begin(&scope).await?;
	let result: Result<bool> = async {
		if !repository::settle_tick(&mut tx, &ended.tenant, ended.tick, ended.status, now).await? {
			return Ok(false);
		}
		super::duty_events::enqueue_completion(&mut tx, ended, now).await?;
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
		| Error::AccountSuspended { .. }
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

/// Whether an error that stopped a Duty's start is a definite refusal, which
/// is recorded with its reason and never retried, rather than one that may
/// pass, which records nothing so the start can be tried again.
pub(super) fn refused(error: &Error) -> bool {
	classify(error).is_some()
}

/// A definite refusal of the Duty Requester's credential or access.
pub(super) fn requester_refused(error: &Error) -> bool {
	matches!(
		error,
		Error::Unauthorized
			| Error::CredentialUnauthorized(_)
			| Error::Forbidden
			| Error::CredentialForbidden(_)
			| Error::AccountSuspended { .. }
	)
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
		Err(error) if requester_refused(&error) => {
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
		// in its Thread; otherwise they lost access there.
		let resource = lease.access_mut().thread(duty.thread_id).await?;
		for action in ["thread.read", "task.create", duties::CREATE] {
			if !lease.access_mut().decide(&resource, action).await? {
				stopping = Some((
					Halt::RequesterLeft,
					format!("the Requester may no longer {action} in this Thread"),
					TickStatus::Failed,
				));
				return Err(Error::Forbidden);
			}
		}
		// Only a refusal stops the Duty; any other error, such as a
		// database outage, leaves the tick to the next pass.
		match crate::apps::execution::capabilities::services::endpoints::readable_thread(
			lease.access_mut(),
			duty.thread_id,
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
		if let Some(reason) = budget_skip(&mut lease, tenant, &duty, now).await? {
			advance(lease.tx(), tenant, &duty, &claim, now).await?;
			skipped_tick(lease.tx(), tenant, &duty, &claim, &reason, now).await?;
			return Ok(Fired::Skipped(reason));
		}
		let task = start(f, &mut lease, &duty, (claim.scheduled_at, "")).await?;
		advance(lease.tx(), tenant, &duty, &claim, now).await?;
		repository::insert_tick(
			lease.tx(),
			tenant,
			duty.thread_id,
			&Tick {
				id: Uuid::new_v4(),
				duty_id: duty.id,
				scheduled_at: claim.scheduled_at,
				recorded_at: now,
				status: TickStatus::Started,
				reason: None,
				task_id: Some(task),
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
		duty.thread_id,
		&Tick {
			id: Uuid::new_v4(),
			duty_id: duty.id,
			scheduled_at: claim.scheduled_at,
			recorded_at: now,
			status: TickStatus::Skipped,
			reason: Some(reason.to_owned()),
			task_id: None,
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
				duty.thread_id,
				&Tick {
					id: Uuid::new_v4(),
					duty_id: duty.id,
					scheduled_at,
					recorded_at: now,
					status,
					reason: Some(reason),
					task_id: None,
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
/// was admitted with, in the Run's Thread. A Run a Duty started may set
/// up Duties too (#1012). Its approval was already decided by that person's
/// Approval Mode and Approval Rules before the call ran.
pub async fn agent_tool(
	f: &Federation,
	run: &aidash_domain::Run,
	key: &str,
	operation: &str,
	input: Value,
) -> Result<Value> {
	use aidash_application::duties::tool;
	use aidash_application::ports::authorization::ExecutionGrantRepository;
	let store = &f.store;
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
			scratch_policy: &f.config.scratch_policy,
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
		// and only those of its Run's Thread: registered from it, or owning
		// it (#1008).
		let own = |duty: &Duty| -> Result<()> {
			if duty.requester == requester
				&& (duty.thread_id == run.thread_id
					|| duty.origin_thread_id == Some(run.thread_id))
			{
				Ok(())
			} else {
				Err(Error::Invalid(
					"an Agent may change only Duties of this Thread that act as the person who started this conversation".into(),
				))
			}
		};
		Ok(match operation {
			"duty_create" => {
				let input: tool::Create =
					serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
				let definition = Definition {
 task_model: input.task_model,
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
					// The Duty tools do not carry scripts yet; a person adds
					// them in the Duty's editor (#1011).
					router: None,
					watcher: None,
				};
				let duty = app::register(
					&mut scope,
					run.thread_id,
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
				"duties": app::list(&mut scope, Listing::Thread(run.thread_id))
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
 task_model: input.task_model,
 title: input.title,
					mission: input.mission,
					procedure: input.procedure,
					agent: input.agent,
					trigger: input.trigger,
					approval_mode: input.approval_mode,
					max_lateness_seconds: input.max_lateness_seconds,
					pause_after_failures: input.pause_after_failures,
					router: None,
					watcher: None,
				};
				summary(&app::update(&mut scope, input.duty_id, patch).await?)
			}
			"duty_delete" => {
				let input: tool::Delete =
					serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
				own(&scope.load(input.duty_id).await?)?;
				match input.action {
					tool::Removal::Delete => {
						app::delete_by_agent(&mut scope, input.duty_id).await?;
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

/// A Duty starts no new Task while a Budget that covers it is at its Hard
/// Limit (#1012 decision 33); the Task it would start is charged to its
/// Node, its Requester and itself.
pub(crate) async fn budget_skip(
	lease: &mut Lease,
	tenant: &str,
	duty: &Duty,
	now: DateTime<Utc>,
) -> Result<Option<String>> {
	let node = repository::thread_node(lease.tx(), tenant, duty.thread_id).await?;
	let charge = crate::apps::identity::repositories::budgets::Charge {
		tenant: tenant.to_owned(),
		node,
		root_subject: duty.requester.clone(),
		duty: duty.id,
	};
	let (gate, _) =
		crate::apps::identity::repositories::budgets::gate_for(lease.tx(), &charge, now).await?;
	Ok(match gate {
		aidash_domain::budgets::Gate::Open => None,
		aidash_domain::budgets::Gate::Stop { budget }
		| aidash_domain::budgets::Gate::Sleep { budget, .. } => {
			Some(format!("{}: {budget}", duties::skip::BUDGET))
		}
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use rstest::rstest;

	#[rstest]
	#[case(Error::Conflict("RUNTIME_UNAVAILABLE: no isolated runner configured".into()), true)]
	#[case(Error::Forbidden, true)]
	#[case(Error::AccountSuspended { epoch: 1 }, true)]
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
