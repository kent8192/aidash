//! Schedule persistence on a caller-owned Tenant transaction (#557).
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::{
	TaskStatus,
	approval_rules::ApprovalMode,
	registry::EntityRef,
	schedules::{Definition, Halt, Limits, Schedule, ThreadMode, Tick, TickStatus},
};
use chrono::{DateTime, Utc};
use reinhardt::db::{backends::TransactionExecutor, orm::Model};
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait, JoinType, LockBehavior, LockType, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder, TableRef,
};
use serde_json::Value;
use uuid::Uuid;

use crate::apps::workspaces::models::{ScheduleLimitRecord, ScheduleRecord, ScheduleTickRecord};
use crate::database::native::{self, Pool};
use crate::{Error, Result};

/// The seed of the advisory lock that serializes a Tenant's registrations.
const SCHEDULE_LOCK: i64 = 557;

fn framework(error: impl Into<reinhardt::core::exception::Error>) -> Error {
	Error::from(error.into())
}

fn mode_text(mode: ApprovalMode) -> Result<String> {
	serde_json::to_value(mode)?
		.as_str()
		.map(str::to_owned)
		.ok_or_else(|| Error::Invalid("invalid Approval Mode".into()))
}

pub(crate) fn domain(row: ScheduleRecord) -> Result<Schedule> {
	Ok(Schedule {
		id: row.id,
		workspace_id: row.workspace_id,
		registrant: row.registrant,
		credential_id: row.credential_id,
		definition: Definition {
			title: row.title,
			request: row.request,
			agent: EntityRef {
				id: row.agent_id,
				version: row.agent_version,
			},
			trigger: serde_json::from_value(row.trigger.0)?,
			thread: row
				.thread_id
				.map_or(ThreadMode::New, |thread_id| ThreadMode::Fixed { thread_id }),
			approval_mode: row
				.approval_mode
				.map(|mode| serde_json::from_value(Value::String(mode)))
				.transpose()?,
			max_lateness_seconds: row.max_lateness_seconds,
		},
		anchor: row.anchor,
		next_scheduled_at: row.next_scheduled_at,
		halt: row.halt.as_deref().map(Halt::parse).transpose()?,
		halted_at: row.halted_at,
		successor: row.successor,
		consecutive_failures: row.consecutive_failures,
		created_by_run_id: row.created_by_run_id,
		revision: row.revision,
		created_at: row.created_at,
		updated_at: row.updated_at,
	})
}

/// Writes every field of `schedule` onto `row`; the caller sets the revision.
fn write(row: &mut ScheduleRecord, schedule: &Schedule) -> Result<()> {
	let definition = &schedule.definition;
	row.workspace_id = schedule.workspace_id;
	row.registrant = schedule.registrant.clone();
	row.credential_id = schedule.credential_id;
	row.title = definition.title.trim().to_owned();
	row.request = definition.request.clone();
	row.agent_id = definition.agent.id.clone();
	row.agent_version = definition.agent.version.clone();
	row.trigger = reinhardt::db::orm::Json(serde_json::to_value(&definition.trigger)?);
	row.thread_id = match definition.thread {
		ThreadMode::New => None,
		ThreadMode::Fixed { thread_id } => Some(thread_id),
	};
	row.approval_mode = definition.approval_mode.map(mode_text).transpose()?;
	row.max_lateness_seconds = definition.max_lateness_seconds;
	row.anchor = schedule.anchor;
	row.next_scheduled_at = schedule.next_scheduled_at;
	row.next_fire_at = schedule.next_fire_at();
	row.halt = schedule.halt.map(|halt| halt.as_str().to_owned());
	row.halted_at = schedule.halted_at;
	row.successor = schedule.successor.clone();
	row.consecutive_failures = schedule.consecutive_failures;
	row.created_by_run_id = schedule.created_by_run_id;
	row.created_at = schedule.created_at;
	row.updated_at = schedule.updated_at;
	Ok(())
}

pub(crate) async fn insert(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	schedule: &Schedule,
) -> Result<()> {
	let mut row = ScheduleRecord::build()
		.id(schedule.id)
		.tenant(tenant.to_owned())
		.workspace_id(schedule.workspace_id)
		.registrant(schedule.registrant.clone())
		.credential_id(schedule.credential_id)
		.title(String::new())
		.request(String::new())
		.agent_id(String::new())
		.agent_version(String::new())
		.trigger(reinhardt::db::orm::Json(Value::Null))
		.thread_id(None)
		.approval_mode(None)
		.max_lateness_seconds(0)
		.anchor(schedule.anchor)
		.next_scheduled_at(None)
		.next_fire_at(None)
		.halt(None)
		.halted_at(None)
		.successor(None)
		.consecutive_failures(0)
		.created_by_run_id(None)
		.revision(0)
		.created_at(schedule.created_at)
		.updated_at(schedule.updated_at)
		.finish();
	write(&mut row, schedule)?;
	ScheduleRecord::objects()
		.insert_with_executor(tx, &row)
		.await
		.map_err(framework)?;
	Ok(())
}

async fn record(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
	lock: bool,
) -> Result<Option<ScheduleRecord>> {
	let query = ScheduleRecord::objects()
		.filter(ScheduleRecord::field_tenant().eq(tenant.to_owned()))
		.filter(ScheduleRecord::field_id().eq(id));
	let rows = if lock {
		query.select_for_update().all_with_executor(tx).await
	} else {
		query.all_with_executor(tx).await
	};
	Ok(rows.map_err(framework)?.pop())
}

pub(crate) async fn load(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
	lock: bool,
) -> Result<Option<Schedule>> {
	record(tx, tenant, id, lock).await?.map(domain).transpose()
}

/// Saves a loaded Schedule at the revision it was read at, and raises it.
pub(crate) async fn save(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	schedule: &Schedule,
) -> Result<Schedule> {
	let mut row = record(tx, tenant, schedule.id, true)
		.await?
		.ok_or_else(|| Error::NotFound("schedule not found".into()))?;
	if row.revision != schedule.revision {
		return Err(Error::Conflict("schedule revision changed".into()));
	}
	write(&mut row, schedule)?;
	row.revision += 1;
	ScheduleRecord::objects()
		.save_with_executor(tx, &row)
		.await
		.map_err(framework)?;
	domain(row)
}

pub(crate) async fn delete(tx: &mut dyn TransactionExecutor, tenant: &str, id: Uuid) -> Result<()> {
	let Some(row) = record(tx, tenant, id, true).await? else {
		return Err(Error::NotFound("schedule not found".into()));
	};
	// Ticks go with their Schedule through the cascading key.
	ScheduleRecord::objects()
		.delete_with_executor(tx, row.id)
		.await
		.map_err(framework)?;
	Ok(())
}

/// Which Schedules a list reads.
pub(crate) enum Filter<'a> {
	Workspace(Uuid),
	Registrant(&'a str),
	Tenant,
}

pub(crate) async fn list(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	filter: Filter<'_>,
) -> Result<Vec<Schedule>> {
	let query =
		ScheduleRecord::objects().filter(ScheduleRecord::field_tenant().eq(tenant.to_owned()));
	let query = match filter {
		Filter::Workspace(workspace) => {
			query.filter(ScheduleRecord::field_workspace_id().eq(workspace))
		}
		Filter::Registrant(subject) => {
			query.filter(ScheduleRecord::field_registrant().eq(subject.to_owned()))
		}
		Filter::Tenant => query,
	};
	query
		.order_by(&["title", "id"])
		.all_with_executor(tx)
		.await
		.map_err(framework)?
		.into_iter()
		.map(domain)
		.collect()
}

/// The Tenant's Schedules and how many `registrant` holds. A transaction
/// lock per Tenant serializes registrations, so two cannot both take the
/// last place; it is advisory, so no Run's lease on the Tenant waits for it.
pub(crate) async fn counts(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	registrant: &str,
) -> Result<(i64, i64)> {
	use reinhardt::db::orm::execution::convert_values;
	use reinhardt::query::{IntoIden, SimpleExpr};
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![SimpleExpr::FunctionCall(
				"hashtextextended".into_iden(),
				vec![
					Expr::value(tenant.to_owned()).into(),
					Expr::value(SCHEDULE_LOCK).into(),
				],
			)],
		))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	let count = |mine: bool| {
		let mut query = Query::select();
		query
			.expr(reinhardt::query::Func::count(Expr::col("id").into()))
			.from(Alias::new("schedules"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()));
		if mine {
			query.and_where(Expr::col("registrant").eq(registrant.to_owned()));
		}
		query.to_string(PostgresQueryBuilder)
	};
	let total = native::query_scalar::<i64>(&count(false))
		.scalar_one(&mut *tx)
		.await?;
	let mine = native::query_scalar::<i64>(&count(true))
		.scalar_one(&mut *tx)
		.await?;
	Ok((total, mine))
}

fn tick_of(row: ScheduleTickRecord) -> Result<Tick> {
	Ok(Tick {
		id: row.id,
		schedule_id: row.schedule_id,
		scheduled_at: row.scheduled_at,
		recorded_at: row.recorded_at,
		status: TickStatus::parse(&row.status)?,
		reason: row.reason,
		task_id: row.task_id,
		thread_id: row.thread_id,
		started_by: row.started_by,
	})
}

pub(crate) async fn ticks(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	schedule: Uuid,
	limit: u32,
) -> Result<Vec<Tick>> {
	ScheduleTickRecord::objects()
		.filter(ScheduleTickRecord::field_tenant().eq(tenant.to_owned()))
		.filter(ScheduleTickRecord::field_schedule_id().eq(schedule))
		.order_by(&["-scheduled_at", "-recorded_at"])
		.limit(limit as usize)
		.all_with_executor(tx)
		.await
		.map_err(framework)?
		.into_iter()
		.map(tick_of)
		.collect()
}

pub(crate) async fn insert_tick(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	workspace: Uuid,
	tick: &Tick,
) -> Result<()> {
	let row = ScheduleTickRecord::build()
		.id(tick.id)
		.tenant(tenant.to_owned())
		.schedule_id(tick.schedule_id)
		.workspace_id(workspace)
		.scheduled_at(tick.scheduled_at)
		.recorded_at(tick.recorded_at)
		.status(tick.status.as_str().to_owned())
		.reason(tick.reason.clone())
		.task_id(tick.task_id)
		.thread_id(tick.thread_id)
		.started_by(tick.started_by.clone())
		.settled_at(None)
		.finish();
	ScheduleTickRecord::objects()
		.insert_with_executor(tx, &row)
		.await
		.map_err(framework)?;
	Ok(())
}

/// The ticks joined with the status of their Tasks.
fn tick_tasks() -> reinhardt::query::SelectStatement {
	let col = |table: &str, name: &str| Expr::col((Alias::new(table), Alias::new(name)));
	Query::select()
		.column((Alias::new("k"), Alias::new("tenant")))
		.column((Alias::new("k"), Alias::new("id")))
		.column((Alias::new("k"), Alias::new("schedule_id")))
		.column((Alias::new("t"), Alias::new("status")))
		.column((Alias::new("k"), Alias::new("started_by")))
		.from_as(Alias::new("schedule_ticks"), Alias::new("k"))
		.join(
			JoinType::InnerJoin,
			TableRef::table_alias(Alias::new("tasks"), Alias::new("t")),
			Condition::all()
				.add(col("t", "tenant").equals((Alias::new("k"), Alias::new("tenant"))))
				.add(col("t", "id").equals((Alias::new("k"), Alias::new("task_id")))),
		)
		.and_where(col("k", "status").eq(TickStatus::Started.as_str()))
		.to_owned()
}

type TickTask = (String, Uuid, Uuid, String, Option<String>);
const TICK_TASK: [&str; 5] = ["tenant", "id", "schedule_id", "status", "started_by"];

fn task_status(status: &str) -> Option<TaskStatus> {
	serde_json::from_value(Value::String(status.to_owned())).ok()
}

/// Whether a scheduled tick of the Schedule still has a Run under way.
pub(crate) async fn running(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	schedule: Uuid,
) -> Result<bool> {
	let open: Vec<TickTask> = native::query_as(
		&tick_tasks()
			.and_where(Expr::col((Alias::new("k"), Alias::new("tenant"))).eq(tenant.to_owned()))
			.and_where(
				Expr::col((Alias::new("k"), Alias::new("schedule_id"))).eq(Expr::value(schedule)),
			)
			.and_where(Expr::col((Alias::new("k"), Alias::new("started_by"))).is_null())
			.to_string(PostgresQueryBuilder),
	)
	.columns(&TICK_TASK)
	.fetch_all(&mut *tx)
	.await?;
	Ok(open
		.iter()
		.any(|(_, _, _, status, _)| task_status(status).and_then(TickStatus::of_task).is_none()))
}

/// A started tick whose Task ended, to be settled in its Tenant's scope.
#[derive(Debug, Clone)]
pub(crate) struct Ended {
	pub tenant: String,
	pub tick: Uuid,
	pub schedule: Uuid,
	pub status: TickStatus,
	/// A "Run now" tick never counts toward the Schedule's failures.
	pub manual: bool,
}

/// Started ticks whose Tasks have ended, across Tenants.
pub(crate) async fn ended(pool: &Pool, limit: u64) -> Result<Vec<Ended>> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::ScheduleTicks);
	let rows: Vec<TickTask> = native::query_as(
		&tick_tasks()
			.and_where(
				Expr::col((Alias::new("t"), Alias::new("status")))
					.is_not_in(["OPEN", "CLAIMED", "RUNNING"]),
			)
			.order_by((Alias::new("k"), Alias::new("recorded_at")), Order::Asc)
			.limit(limit)
			.to_string(PostgresQueryBuilder),
	)
	.columns(&TICK_TASK)
	.fetch_all(pool.in_scope(&scope))
	.await?;
	Ok(rows
		.into_iter()
		.filter_map(|(tenant, tick, schedule, status, started_by)| {
			Some(Ended {
				tenant,
				tick,
				schedule,
				status: TickStatus::of_task(task_status(&status)?)?,
				manual: started_by.is_some(),
			})
		})
		.collect())
}

/// Settles one ended tick. Whether it was still open: another replica may
/// have settled it first.
pub(crate) async fn settle_tick(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	tick: Uuid,
	status: TickStatus,
	now: DateTime<Utc>,
) -> Result<bool> {
	let Some(mut row) = ScheduleTickRecord::objects()
		.filter(ScheduleTickRecord::field_tenant().eq(tenant.to_owned()))
		.filter(ScheduleTickRecord::field_id().eq(tick))
		.select_for_update()
		.all_with_executor(tx)
		.await
		.map_err(framework)?
		.pop()
	else {
		return Ok(false);
	};
	if row.status != TickStatus::Started.as_str() {
		return Ok(false);
	}
	row.status = status.as_str().to_owned();
	row.settled_at = Some(now);
	ScheduleTickRecord::objects()
		.save_with_executor(tx, &row)
		.await
		.map_err(framework)?;
	Ok(true)
}

/// Due Schedules across Tenants, oldest first. Each is claimed again, with
/// `SKIP LOCKED`, in its Registrant's own transaction.
pub(crate) async fn due(
	pool: &Pool,
	now: DateTime<Utc>,
	limit: u64,
) -> Result<Vec<(String, Uuid)>> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::ScheduleTicks);
	let rows: Vec<(String, Uuid)> = native::query_as(
		&Query::select()
			.columns([Alias::new("tenant"), Alias::new("id")])
			.from(Alias::new("schedules"))
			.and_where(Expr::col("halt").is_null())
			.and_where(Expr::col("next_fire_at").lte(Expr::value(now)))
			// A deleted Tenant's Schedules are stopped with it; never start one
			// in the moment between the deletion and that stop.
			.and_where(
				Expr::exists(
					Query::select()
						.expr(Expr::value(1_i32))
						.from(Alias::new("tenant_deletions"))
						.and_where(
							Expr::col((Alias::new("tenant_deletions"), Alias::new("tenant")))
								.equals((Alias::new("schedules"), Alias::new("tenant"))),
						)
						.and_where(Expr::col("restored_at").is_null())
						.to_owned(),
				)
				.not(),
			)
			.order_by(Alias::new("next_fire_at"), Order::Asc)
			.limit(limit)
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["tenant", "id"])
	.fetch_all(pool.in_scope(&scope))
	.await?;
	Ok(rows)
}

/// Claims a due Schedule for this transaction, or None when another replica
/// holds it or already moved it on. The lock lasts until the transaction ends.
pub(crate) async fn claim(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
	now: DateTime<Utc>,
) -> Result<Option<Schedule>> {
	let ids: Vec<Uuid> = native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("schedules"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("halt").is_null())
			.and_where(Expr::col("next_fire_at").lte(Expr::value(now)))
			.lock(LockType::Update)
			.lock_behavior(LockBehavior::SkipLocked)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&mut *tx)
	.await?;
	if ids.is_empty() {
		return Ok(None);
	}
	load(tx, tenant, id, false).await
}

/// What stops Schedules besides a person.
pub(crate) enum Stop<'a> {
	/// Their Registrant left the Tenant or lost access.
	Registrant(&'a str),
	/// Their fixed Thread was deleted.
	Thread(Uuid),
	/// The Tenant was deleted.
	Tenant,
}

/// Stops the Schedules `stop` reaches and returns them. A Schedule already
/// waiting for a successor, or stopped with the Tenant, keeps that reason.
pub(crate) async fn stop(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	stop: Stop<'_>,
	now: DateTime<Utc>,
) -> Result<Vec<Schedule>> {
	let query =
		ScheduleRecord::objects().filter(ScheduleRecord::field_tenant().eq(tenant.to_owned()));
	let (query, halt) = match stop {
		Stop::Registrant(subject) => (
			query.filter(ScheduleRecord::field_registrant().eq(subject.to_owned())),
			Halt::RegistrantLeft,
		),
		Stop::Thread(thread) => (
			query.filter(ScheduleRecord::field_thread_id().eq(thread)),
			Halt::ThreadDeleted,
		),
		Stop::Tenant => (query, Halt::TenantDeleted),
	};
	let rows = query
		.select_for_update()
		.all_with_executor(tx)
		.await
		.map_err(framework)?;
	let mut stopped = vec![];
	for row in rows {
		let mut schedule = domain(row)?;
		let keep = match halt {
			// A paused Schedule stays paused when its Thread goes; resuming
			// it then refuses until the Thread is changed.
			Halt::ThreadDeleted => schedule.halt.is_some(),
			Halt::RegistrantLeft => matches!(
				schedule.halt,
				Some(Halt::RegistrantLeft | Halt::TenantDeleted)
			),
			_ => schedule.halt == Some(Halt::TenantDeleted),
		};
		if keep {
			continue;
		}
		schedule.stop(halt, now);
		schedule.updated_at = now;
		stopped.push(save(tx, tenant, &schedule).await?);
	}
	Ok(stopped)
}

pub(crate) async fn limits(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<Limits> {
	Ok(ScheduleLimitRecord::objects()
		.filter(ScheduleLimitRecord::field_tenant().eq(tenant.to_owned()))
		.all_with_executor(tx)
		.await
		.map_err(framework)?
		.pop()
		.map_or_else(Limits::default, |row| Limits {
			max_schedules: row.max_schedules,
			max_per_person: row.max_per_person,
			min_interval_seconds: row.min_interval_seconds,
		}))
}

pub(crate) async fn put_limits(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	updated_by: &str,
	limits: &Limits,
	now: DateTime<Utc>,
) -> Result<()> {
	let existing = ScheduleLimitRecord::objects()
		.filter(ScheduleLimitRecord::field_tenant().eq(tenant.to_owned()))
		.select_for_update()
		.all_with_executor(tx)
		.await
		.map_err(framework)?
		.pop();
	let objects = ScheduleLimitRecord::objects();
	match existing {
		Some(mut row) => {
			row.max_schedules = limits.max_schedules;
			row.max_per_person = limits.max_per_person;
			row.min_interval_seconds = limits.min_interval_seconds;
			row.revision += 1;
			row.updated_at = now;
			row.updated_by = updated_by.to_owned();
			objects
				.save_with_executor(tx, &row)
				.await
				.map_err(framework)?;
		}
		None => {
			let row = ScheduleLimitRecord::build()
				.tenant(tenant.to_owned())
				.max_schedules(limits.max_schedules)
				.max_per_person(limits.max_per_person)
				.min_interval_seconds(limits.min_interval_seconds)
				.revision(0)
				.updated_at(now)
				.updated_by(updated_by.to_owned())
				.finish();
			objects
				.insert_with_executor(tx, &row)
				.await
				.map_err(framework)?;
		}
	}
	Ok(())
}

/// Tasks Schedules started in the last week, of the given Schedules' ticks.
pub(crate) async fn recent_tasks(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	schedules: &[Uuid],
	since: DateTime<Utc>,
) -> Result<Vec<Uuid>> {
	if schedules.is_empty() {
		return Ok(vec![]);
	}
	Ok(ScheduleTickRecord::objects()
		.filter(ScheduleTickRecord::field_tenant().eq(tenant.to_owned()))
		.filter(ScheduleTickRecord::field_schedule_id().is_in(schedules.to_vec()))
		.filter(ScheduleTickRecord::field_recorded_at().gte(since))
		.filter(ScheduleTickRecord::field_task_id().is_not_null())
		.filter(ScheduleTickRecord::field_started_by().is_null())
		.all_with_executor(tx)
		.await
		.map_err(framework)?
		.into_iter()
		.filter_map(|row| row.task_id)
		.collect())
}

/// Whether `task` is a Schedule's tick, or descends from one through its
/// parents (#557 decision 7).
pub(crate) async fn scheduled_chain(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	task: Uuid,
) -> Result<bool> {
	let mut current = Some(task);
	// The parent chain is bounded by the delegation depth; stop well past it.
	for _ in 0..64 {
		let Some(id) = current else {
			return Ok(false);
		};
		let ticked: Option<Uuid> = native::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("schedule_ticks"))
				.and_where(Expr::col("tenant").eq(tenant.to_owned()))
				.and_where(Expr::col("task_id").eq(Expr::value(id)))
				// "Run now" starts an ordinary Run of the person who clicked it.
				.and_where(Expr::col("started_by").is_null())
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut *tx)
		.await?;
		if ticked.is_some() {
			return Ok(true);
		}
		current = native::query_scalar::<Option<Uuid>>(
			&Query::select()
				.column(Alias::new("parent_id"))
				.from(Alias::new("tasks"))
				.and_where(Expr::col("tenant").eq(tenant.to_owned()))
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut *tx)
		.await?
		.flatten();
	}
	Ok(true)
}

/// The Thread a Task's Run works in, if it is bound to one.
pub(crate) async fn task_thread(
	tx: &mut dyn TransactionExecutor,
	task: Uuid,
) -> Result<Option<Uuid>> {
	Ok(native::query_scalar::<Uuid>(
		&Query::select()
			.column(Alias::new("thread_id"))
			.from(Alias::new("core_task_sessions"))
			.and_where(Expr::col("task_id").eq(Expr::value(task)))
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut *tx)
	.await?)
}

/// When the Schedule's latest scheduled tick was due; "Run now" ticks aside.
pub(crate) async fn last_scheduled(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	schedule: Uuid,
) -> Result<Option<DateTime<Utc>>> {
	Ok(native::query_scalar::<DateTime<Utc>>(
		&Query::select()
			.column(Alias::new("scheduled_at"))
			.from(Alias::new("schedule_ticks"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("schedule_id").eq(Expr::value(schedule)))
			.and_where(Expr::col("started_by").is_null())
			.order_by(Alias::new("scheduled_at"), Order::Desc)
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut *tx)
	.await?)
}
