//! Duty persistence on a caller-owned Tenant transaction (#557).
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::{
	TaskStatus,
	approval_rules::ApprovalMode,
	duties::{Definition, Duty, Halt, Tick, TickStatus},
	registry::EntityRef,
};
use chrono::{DateTime, Utc};
use reinhardt::db::{backends::TransactionExecutor, orm::Model};
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait, Func, JoinType, LockBehavior, LockType, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder, TableRef,
};
use serde_json::Value;
use uuid::Uuid;

use crate::apps::workspaces::models::{DutyRecord, DutyTickRecord};
use crate::database::native::{self, Pool};
use crate::{Error, Result};

/// The seed of the advisory lock that serializes a Tenant's registrations.
const DUTY_LOCK: i64 = 557;

fn framework(error: impl Into<reinhardt::core::exception::Error>) -> Error {
	Error::from(error.into())
}

fn mode_text(mode: ApprovalMode) -> Result<String> {
	serde_json::to_value(mode)?
		.as_str()
		.map(str::to_owned)
		.ok_or_else(|| Error::Invalid("invalid Approval Mode".into()))
}

pub(crate) fn domain(row: DutyRecord) -> Result<Duty> {
	Ok(Duty {
		id: row.id,
		thread_id: row.thread_id,
		origin_thread_id: row.origin_thread_id,
		requester: row.requester,
		credential_id: row.credential_id,
		definition: Definition {
			title: row.title,
			mission: row.mission,
			procedure: row.procedure,
			agent: EntityRef {
				id: row.agent_id,
				version: row.agent_version,
			},
			trigger: row
				.trigger
				.map(|trigger| serde_json::from_value(trigger.0))
				.transpose()?,
			approval_mode: row
				.approval_mode
				.map(|mode| serde_json::from_value(Value::String(mode)))
				.transpose()?,
			max_lateness_seconds: row.max_lateness_seconds,
			pause_after_failures: row.pause_after_failures,
			router: row
				.router
				.map(|router| serde_json::from_value(router.0))
				.transpose()?,
			watcher: row
				.watcher
				.map(|watcher| serde_json::from_value(watcher.0))
				.transpose()?,
		},
		anchor: row.anchor,
		next_scheduled_at: row.next_scheduled_at,
		watcher_next_at: row.watcher_next_at,
		halt: row.halt.as_deref().map(Halt::parse).transpose()?,
		halted_at: row.halted_at,
		halt_reason: row.halt_reason,
		successor: row.successor,
		consecutive_failures: row.consecutive_failures,
		created_by_run_id: row.created_by_run_id,
		creation_key: row.creation_key,
		template: row
			.template
			.map(|template| serde_json::from_value(template.0))
			.transpose()?,
		revision: row.revision,
		created_at: row.created_at,
		updated_at: row.updated_at,
	})
}

/// Writes every field of `duty` onto `row`; the caller sets the revision.
fn write(row: &mut DutyRecord, duty: &Duty) -> Result<()> {
	let definition = &duty.definition;
	row.thread_id = duty.thread_id;
	row.origin_thread_id = duty.origin_thread_id;
	row.requester = duty.requester.clone();
	row.credential_id = duty.credential_id;
	row.title = definition.title.trim().to_owned();
	row.mission = definition.mission.clone();
	row.procedure = definition.procedure.clone();
	row.agent_id = definition.agent.id.clone();
	row.agent_version = definition.agent.version.clone();
	row.trigger = definition
		.trigger
		.as_ref()
		.map(|trigger| serde_json::to_value(trigger).map(reinhardt::db::orm::Json))
		.transpose()?;
	row.approval_mode = definition.approval_mode.map(mode_text).transpose()?;
	row.max_lateness_seconds = definition.max_lateness_seconds;
	row.pause_after_failures = definition.pause_after_failures;
	row.router = definition
		.router
		.as_ref()
		.map(|router| serde_json::to_value(router).map(reinhardt::db::orm::Json))
		.transpose()?;
	row.watcher = definition
		.watcher
		.as_ref()
		.map(|watcher| serde_json::to_value(watcher).map(reinhardt::db::orm::Json))
		.transpose()?;
	row.watcher_next_at = duty.watcher_next_at;
	row.anchor = duty.anchor;
	row.next_scheduled_at = duty.next_scheduled_at;
	row.next_fire_at = duty.next_fire_at();
	row.halt = duty.halt.map(|halt| halt.as_str().to_owned());
	row.halted_at = duty.halted_at;
	row.halt_reason = duty.halt_reason.clone();
	row.successor = duty.successor.clone();
	row.consecutive_failures = duty.consecutive_failures;
	row.created_by_run_id = duty.created_by_run_id;
	row.creation_key = duty.creation_key.clone();
	row.template = duty
		.template
		.as_ref()
		.map(|template| serde_json::to_value(template).map(reinhardt::db::orm::Json))
		.transpose()?;
	row.created_at = duty.created_at;
	row.updated_at = duty.updated_at;
	Ok(())
}

pub(crate) async fn insert(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: &Duty,
) -> Result<()> {
	let mut row = DutyRecord::build()
		.id(duty.id)
		.tenant(tenant.to_owned())
		.thread_id(duty.thread_id)
		.origin_thread_id(duty.origin_thread_id)
		.requester(duty.requester.clone())
		.credential_id(duty.credential_id)
		.title(String::new())
		.mission(String::new())
		.procedure(String::new())
		.agent_id(String::new())
		.agent_version(String::new())
		.trigger(None)
		.approval_mode(None)
		.max_lateness_seconds(0)
		.pause_after_failures(0)
		.anchor(duty.anchor)
		.next_scheduled_at(None)
		.next_fire_at(None)
		.halt(None)
		.halted_at(None)
		.halt_reason(None)
		.successor(None)
		.consecutive_failures(0)
		.created_by_run_id(None)
		.creation_key(None)
		.revision(0)
		.created_at(duty.created_at)
		.updated_at(duty.updated_at)
		.router(None)
		.watcher(None)
		.watcher_next_at(None)
		.template(None)
		.finish();
	write(&mut row, duty)?;
	DutyRecord::objects()
		.insert_with_executor(tx, &row)
		.await
		.map_err(framework)?;
	creating_task(tx, tenant, duty).await
}

/// The Duty whose own Thread `thread` is, as a scalar subquery (#1008, ADR
/// 0140). Every owner lookup of a Thread selects it beside `owner_subject`,
/// in the same statement, so the `thread` resource's `duty_id` costs no
/// statement and agrees on reads, lists and `/api/state`.
///
/// Only a Duty that opened its Thread counts (`origin_thread_id` is set at
/// registration). A Duty registered before #1008 kept the former Workspace it
/// was registered in, which people and other Duties share, so that Thread
/// names no Duty. A registration always opens a new Thread, so at most one
/// row matches; the order and limit only keep the expression scalar.
pub(crate) fn own_duty(
	thread: reinhardt::query::SimpleExpr,
	tenant: reinhardt::query::SimpleExpr,
) -> reinhardt::query::SimpleExpr {
	let duty = || Alias::new("own_duty");
	let mut query = Query::select();
	query
		.column((duty(), Alias::new("id")))
		.from_as(Alias::new("duties"), duty())
		.and_where(Expr::col((duty(), Alias::new("thread_id"))).eq(thread))
		.and_where(Expr::col((duty(), Alias::new("tenant"))).eq(tenant))
		.and_where(Expr::col((duty(), Alias::new("origin_thread_id"))).is_not_null())
		.order_by((duty(), Alias::new("id")), Order::Asc)
		.limit(1);
	reinhardt::query::SimpleExpr::SubQuery(None, Box::new(query.to_owned()))
}

/// Records, or clears, the Task whose Run set the Duty up (#1012; see
/// threads 0047): a flow's stop pauses the Duties its Runs set up. A
/// successor taking the Duty over clears the creating Run, and with it this.
/// A Duty a Duty's Run sets up also sits below that Duty in the Budget
/// lineage (identity 0112), which a successor does not clear.
async fn creating_task(tx: &mut dyn TransactionExecutor, tenant: &str, duty: &Duty) -> Result<()> {
	if let Some(run) = duty.created_by_run_id {
		creating_duty(tx, tenant, duty.id, run).await?;
	}
	let task = match duty.created_by_run_id {
		Some(run) => {
			let mut task = Query::select();
			task.column(Alias::new("task_id"))
				.from(Alias::new("task_executions"))
				.and_where(Expr::col("tenant").eq(tenant.to_owned()))
				.and_where(Expr::col("id").eq(Expr::value(run)));
			reinhardt::query::SimpleExpr::SubQuery(None, Box::new(task.to_owned()))
		}
		None => Expr::value(Option::<Uuid>::None).into(),
	};
	native::query(
		&Query::update()
			.table(Alias::new("duties"))
			.value_expr(Alias::new("created_by_task_id"), task)
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("id").eq(Expr::value(duty.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	Ok(())
}

/// Puts `duty` below the Duty whose Task's Run `run` set it up, if a Duty's
/// did, so that Duty's Budgets cover it (#1012 decision 33).
async fn creating_duty(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: Uuid,
	run: Uuid,
) -> Result<()> {
	let (r, t) = (Alias::new("r"), Alias::new("t"));
	let parent: Option<Uuid> = native::query_scalar(
		&Query::select()
			.column((t.clone(), Alias::new("duty_id")))
			.from_as(Alias::new("task_executions"), r.clone())
			.join(
				reinhardt::query::JoinType::InnerJoin,
				reinhardt::query::TableRef::table_alias(Alias::new("tasks"), t.clone()),
				Expr::col((t.clone(), Alias::new("id")))
					.equals((r.clone(), Alias::new("task_id")))
					.and(
						Expr::col((t.clone(), Alias::new("tenant")))
							.equals((r.clone(), Alias::new("tenant"))),
					),
			)
			.and_where(Expr::col((r.clone(), Alias::new("tenant"))).eq(tenant.to_owned()))
			.and_where(Expr::col((r, Alias::new("id"))).eq(Expr::value(run)))
			.and_where(Expr::col((t, Alias::new("duty_id"))).is_not_null())
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut *tx)
	.await?;
	if let Some(parent) = parent {
		crate::apps::identity::repositories::budgets::record_lineage(
			tx,
			tenant,
			crate::apps::identity::repositories::budgets::Lineage::Duty,
			&duty.to_string(),
			&parent.to_string(),
		)
		.await?;
	}
	Ok(())
}

/// The Duties Runs of `tasks` set up, or Runs of `runs` before threads
/// 0047 recorded the creating Task (#1012): what a flow's stop pauses.
pub(crate) async fn created_by_flow(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	tasks: &[Uuid],
	runs: &[Uuid],
) -> Result<Vec<Uuid>> {
	if tasks.is_empty() && runs.is_empty() {
		return Ok(vec![]);
	}
	let mut by = Condition::any();
	if !tasks.is_empty() {
		by = by.add(
			Expr::col("created_by_task_id").is_in(tasks.iter().map(|task| Expr::value(*task))),
		);
	}
	if !runs.is_empty() {
		by = by.add(Expr::col("created_by_run_id").is_in(runs.iter().map(|run| Expr::value(*run))));
	}
	native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("duties"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.cond_where(by)
			.order_by(Alias::new("id"), Order::Asc)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&mut *tx)
	.await
}

/// The Task whose Run set up each of the Tenant's Duties, when one did.
pub(crate) async fn creating_tasks(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
) -> Result<std::collections::BTreeMap<Uuid, Uuid>> {
	let rows: Vec<(Uuid, Uuid)> = native::query_as(
		&Query::select()
			.columns([Alias::new("id"), Alias::new("created_by_task_id")])
			.from(Alias::new("duties"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("created_by_task_id").is_not_null())
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["id", "created_by_task_id"])
	.fetch_all(&mut *tx)
	.await?;
	Ok(rows.into_iter().collect())
}

async fn record(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
	lock: bool,
) -> Result<Option<DutyRecord>> {
	let query = DutyRecord::objects()
		.filter(DutyRecord::field_tenant().eq(tenant.to_owned()))
		.filter(DutyRecord::field_id().eq(id));
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
) -> Result<Option<Duty>> {
	record(tx, tenant, id, lock).await?.map(domain).transpose()
}

/// Associates the replacement with the already locked Duty so the Thread
/// resource lookup can read the Duty ID before the final save.
pub(crate) async fn bind_replacement_thread(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: &Duty,
	deleted: Uuid,
) -> Result<()> {
	let updated = native::query(
		&Query::update()
			.table(Alias::new("duties"))
			.value(Alias::new("thread_id"), Expr::value(duty.thread_id))
			.value(
				Alias::new("origin_thread_id"),
				Expr::value(duty.origin_thread_id),
			)
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("id").eq(Expr::value(duty.id)))
			.and_where(Expr::col("thread_id").eq(Expr::value(deleted)))
			.and_where(Expr::col("revision").eq(duty.revision))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	if updated.rows_affected() != 1 {
		return Err(Error::Conflict("duty revision changed".into()));
	}
	Ok(())
}

/// Saves a loaded Duty at the revision it was read at, and raises it.
pub(crate) async fn save(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: &Duty,
) -> Result<Duty> {
	let mut row = record(tx, tenant, duty.id, true)
		.await?
		.ok_or_else(|| Error::NotFound("duty not found".into()))?;
	if row.revision != duty.revision {
		return Err(Error::Conflict("duty revision changed".into()));
	}
	let taken_over = row.created_by_run_id.is_some() && duty.created_by_run_id.is_none();
	write(&mut row, duty)?;
	row.revision += 1;
	DutyRecord::objects()
		.save_with_executor(tx, &row)
		.await
		.map_err(framework)?;
	if taken_over {
		creating_task(tx, tenant, duty).await?;
	}
	domain(row)
}

pub(crate) async fn delete(tx: &mut dyn TransactionExecutor, tenant: &str, id: Uuid) -> Result<()> {
	let Some(row) = record(tx, tenant, id, true).await? else {
		return Err(Error::NotFound("duty not found".into()));
	};
	// Its open Router and Watcher Tasks stop before their script rows go
	// with it (#1011).
	super::duty_scripts::cancel_open(tx, tenant, row.id).await?;
	preserve_flow_ancestry(tx, tenant, id).await?;
	// Ticks go with their Duty through the cascading key; its Budgets go
	// too, while its spend stays in the ledger (#1012).
	crate::apps::identity::repositories::budgets::delete_for_duty(tx, tenant, row.id).await?;
	DutyRecord::objects()
		.delete_with_executor(tx, row.id)
		.await
		.map_err(framework)?;
	Ok(())
}

/// Keep the creating flow's link to Tasks that outlive their Duty. Older
/// Duties may identify their creator only by Run.
async fn preserve_flow_ancestry(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: Uuid,
) -> Result<()> {
	let (d, r) = (Alias::new("d"), Alias::new("r"));
	let parent: Option<Uuid> = native::query_scalar(
		&Query::select()
			.expr(Func::coalesce(vec![
				Expr::col((d.clone(), Alias::new("created_by_task_id"))).into(),
				Expr::col((r.clone(), Alias::new("task_id"))).into(),
			]))
			.from_as(Alias::new("duties"), d.clone())
			.join(
				JoinType::LeftJoin,
				TableRef::table_alias(Alias::new("task_executions"), r.clone()),
				Expr::col((r.clone(), Alias::new("id")))
					.equals((d.clone(), Alias::new("created_by_run_id")))
					.and(
						Expr::col((r, Alias::new("tenant")))
							.equals((d.clone(), Alias::new("tenant"))),
					),
			)
			.and_where(Expr::col((d.clone(), Alias::new("tenant"))).eq(tenant))
			.and_where(Expr::col((d, Alias::new("id"))).eq(Expr::value(duty)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut *tx)
	.await?;
	if let Some(parent) = parent {
		native::query(
			&Query::update()
				.table(Alias::new("tasks"))
				.value_expr(Alias::new("deleted_duty_parent_id"), Expr::value(parent))
				.and_where(Expr::col("tenant").eq(tenant))
				.and_where(Expr::col("duty_id").eq(Expr::value(duty)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
	}
	Ok(())
}

/// Who last set each Budget that targets the Duty (#1012). Deleting the
/// Duty removes them.
pub(crate) async fn budget_setters(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: Uuid,
) -> Result<Vec<String>> {
	native::query_scalar(
		&Query::select()
			.column(Alias::new("created_by"))
			.from(Alias::new("budgets"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("scope_kind").eq("duty"))
			.and_where(Expr::col("scope_key").eq(duty.to_string()))
			.order_by(Alias::new("created_by"), Order::Asc)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&mut *tx)
	.await
}

/// Makes a Duty's own Thread its successor's (#1011): whoever takes a Duty
/// over owns the Thread it works in, so they may manage and delete it as
/// its Requester did.
pub(crate) async fn take_over_thread(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	thread: Uuid,
	successor: &str,
) -> Result<()> {
	native::query(
		&Query::update()
			.table(Alias::new("authorization_threads"))
			.value(Alias::new("owner_subject"), successor.to_owned())
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("thread_id").eq(Expr::value(thread)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	Ok(())
}

/// The Threads among `threads` that were deleted.
pub(crate) async fn deleted_threads(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	threads: &[Uuid],
) -> Result<std::collections::HashSet<Uuid>> {
	if threads.is_empty() {
		return Ok(Default::default());
	}
	let deleted: Vec<Uuid> = native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("threads"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("id").is_in(threads.iter().copied().map(Expr::value)))
			.and_where(Expr::col("deleted_at").is_not_null())
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&mut *tx)
	.await?;
	Ok(deleted.into_iter().collect())
}

/// Which Duties a list reads.
pub(crate) enum Filter<'a> {
	Thread(Uuid),
	/// The person's own Duties, and those waiting for them to take over
	/// as the named successor.
	Mine(&'a str),
	Tenant,
}

pub(crate) async fn list(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	filter: Filter<'_>,
) -> Result<Vec<Duty>> {
	let query = DutyRecord::objects().filter(DutyRecord::field_tenant().eq(tenant.to_owned()));
	let query = match filter {
		Filter::Thread(thread) => query.filter(
			DutyRecord::field_thread_id()
				.eq(thread)
				.or(DutyRecord::field_origin_thread_id().eq(Some(thread))),
		),
		Filter::Mine(subject) => query.filter(
			DutyRecord::field_requester()
				.eq(subject.to_owned())
				.or(DutyRecord::field_successor().eq(Some(subject.to_owned()))),
		),
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

/// The Tenant's Duties. A transaction lock per Tenant serializes
/// registrations, so two cannot both take the last place a plan allows; it is
/// advisory, so no Run's lease on the Tenant waits for it.
pub(crate) async fn count(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<i64> {
	lock_registrations(tx, tenant).await?;
	native::query_scalar::<i64>(
		&Query::select()
			.expr(reinhardt::query::Func::count(Expr::col("id").into()))
			.from(Alias::new("duties"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut *tx)
	.await
}

/// Takes the transaction lock that serializes a Tenant's registrations.
pub(crate) async fn lock_registrations(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
) -> Result<()> {
	use reinhardt::db::orm::execution::convert_values;
	use reinhardt::query::{IntoIden, SimpleExpr};
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![SimpleExpr::FunctionCall(
				"hashtextextended".into_iden(),
				vec![
					Expr::value(tenant.to_owned()).into(),
					Expr::value(DUTY_LOCK).into(),
				],
			)],
		))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

/// The Duty `requester` installed from a template with this install request
/// (#1011 D4), so a retried install returns it. The caller holds
/// [`lock_registrations`], so two retries cannot both miss it.
pub(crate) async fn installed(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	requester: &str,
	install_key: Uuid,
) -> Result<Option<Duty>> {
	let id = native::query_scalar::<Uuid>(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("duties"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("requester").eq(requester.to_owned()))
			.and_where(Expr::cust_with_values(
				"template->>'install_key' = ?",
				[install_key.to_string()],
			))
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut *tx)
	.await?;
	match id {
		Some(id) => load(tx, tenant, id, false).await,
		None => Ok(None),
	}
}

/// The mission and procedure of the Duty that started `task`, if one did;
/// its Run works under them (#1011). The Task records its Duty before its Run
/// is claimed. Tasks its Runs create record the Duty too, for Budgets, but
/// only the Task the Duty started itself, at the root of the Duty's own
/// Thread and created by no Run, works under its brief.
pub(crate) async fn brief_of_task(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	task: Uuid,
) -> Result<Option<String>> {
	let (duties, tasks) = (Alias::new("duties"), Alias::new("tasks"));
	let row = native::query(
		&Query::select()
			.column((duties.clone(), Alias::new("mission")))
			.column((duties.clone(), Alias::new("procedure")))
			.from(tasks.clone())
			.inner_join(
				duties.clone(),
				Condition::all()
					.add(
						Expr::col((duties.clone(), Alias::new("id")))
							.equals((tasks.clone(), Alias::new("duty_id"))),
					)
					.add(
						Expr::col((duties.clone(), Alias::new("tenant")))
							.equals((tasks.clone(), Alias::new("tenant"))),
					)
					.add(
						Expr::col((duties, Alias::new("thread_id")))
							.equals((tasks.clone(), Alias::new("thread_id"))),
					),
			)
			.and_where(Expr::col((tasks.clone(), Alias::new("tenant"))).eq(tenant.to_owned()))
			.and_where(Expr::col((tasks.clone(), Alias::new("id"))).eq(Expr::value(task)))
			.and_where(Expr::col((tasks.clone(), Alias::new("parent_id"))).is_null())
			.and_where(
				Expr::exists(
					Query::select()
						.expr(Expr::value(1_i32))
						.from(Alias::new("authorization_task_origins"))
						.and_where(
							Expr::col((
								Alias::new("authorization_task_origins"),
								Alias::new("task_id"),
							))
							.equals((tasks.clone(), Alias::new("id"))),
						)
						.and_where(
							Expr::col((
								Alias::new("authorization_task_origins"),
								Alias::new("tenant"),
							))
							.equals((tasks, Alias::new("tenant"))),
						)
						.to_owned(),
				)
				.not(),
			)
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut *tx)
	.await?;
	Ok(match row {
		Some(row) => Some(
			aidash_domain::standing_instructions::StandingInstructions::duty_text(
				&row.try_get::<String>("mission")?,
				&row.try_get::<String>("procedure")?,
			),
		),
		None => None,
	})
}

fn tick_of(row: DutyTickRecord) -> Result<Tick> {
	Ok(Tick {
		id: row.id,
		duty_id: row.duty_id,
		scheduled_at: row.scheduled_at,
		recorded_at: row.recorded_at,
		status: TickStatus::parse(&row.status)?,
		reason: row.reason,
		task_id: row.task_id,
		started_by: row.started_by,
	})
}

pub(crate) async fn ticks(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: Uuid,
	limit: u32,
) -> Result<Vec<Tick>> {
	DutyTickRecord::objects()
		.filter(DutyTickRecord::field_tenant().eq(tenant.to_owned()))
		.filter(DutyTickRecord::field_duty_id().eq(duty))
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
	thread: Uuid,
	tick: &Tick,
) -> Result<()> {
	let row = DutyTickRecord::build()
		.id(tick.id)
		.tenant(tenant.to_owned())
		.duty_id(tick.duty_id)
		.thread_id(thread)
		.scheduled_at(tick.scheduled_at)
		.recorded_at(tick.recorded_at)
		.status(tick.status.as_str().to_owned())
		.reason(tick.reason.clone())
		.task_id(tick.task_id)
		.started_by(tick.started_by.clone())
		.settled_at(None)
		.finish();
	DutyTickRecord::objects()
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
		.column((Alias::new("k"), Alias::new("duty_id")))
		.column((Alias::new("t"), Alias::new("status")))
		.column((Alias::new("k"), Alias::new("started_by")))
		.column((Alias::new("k"), Alias::new("task_id")))
		.from_as(Alias::new("duty_ticks"), Alias::new("k"))
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

type TickTask = (String, Uuid, Uuid, String, Option<String>, Uuid);
const TICK_TASK: [&str; 6] = ["tenant", "id", "duty_id", "status", "started_by", "task_id"];

fn task_status(status: &str) -> Option<TaskStatus> {
	serde_json::from_value(Value::String(status.to_owned())).ok()
}

/// Whether a scheduled tick of the Duty still has a Run under way.
pub(crate) async fn running(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: Uuid,
) -> Result<bool> {
	let open: Vec<TickTask> = native::query_as(
		&tick_tasks()
			.and_where(Expr::col((Alias::new("k"), Alias::new("tenant"))).eq(tenant.to_owned()))
			.and_where(Expr::col((Alias::new("k"), Alias::new("duty_id"))).eq(Expr::value(duty)))
			.and_where(Expr::col((Alias::new("k"), Alias::new("started_by"))).is_null())
			.to_string(PostgresQueryBuilder),
	)
	.columns(&TICK_TASK)
	.fetch_all(&mut *tx)
	.await?;
	Ok(open
		.iter()
		.any(|(_, _, _, status, _, _)| task_status(status).and_then(TickStatus::of_task).is_none()))
}

/// A started tick whose Task ended, to be settled in its Tenant's scope.
#[derive(Debug, Clone)]
pub(crate) struct Ended {
	pub tenant: String,
	pub tick: Uuid,
	pub duty: Uuid,
	pub task: Uuid,
	pub status: TickStatus,
	/// A "Run now" tick never counts toward the Duty's failures.
	pub manual: bool,
}

/// Started ticks whose Tasks have ended, across Tenants.
pub(crate) async fn ended(pool: &Pool, limit: u64) -> Result<Vec<Ended>> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::DutyTicks);
	let rows: Vec<TickTask> = native::query_as(
		&tick_tasks()
			.and_where(
				Expr::col((Alias::new("t"), Alias::new("status")))
					.is_not_in(["OPEN", "CLAIMED", "RUNNING", "BLOCKED"]),
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
		.filter_map(|(tenant, tick, duty, status, started_by, task)| {
			Some(Ended {
				tenant,
				tick,
				duty,
				task,
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
	let Some(mut row) = DutyTickRecord::objects()
		.filter(DutyTickRecord::field_tenant().eq(tenant.to_owned()))
		.filter(DutyTickRecord::field_id().eq(tick))
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
	DutyTickRecord::objects()
		.save_with_executor(tx, &row)
		.await
		.map_err(framework)?;
	Ok(true)
}

/// Due Duties across Tenants, oldest first, after the `(next_fire_at,
/// id)` a previous page ended at. Each is claimed again, with `SKIP LOCKED`,
/// in its Requester's own transaction.
pub(crate) async fn due(
	pool: &Pool,
	now: DateTime<Utc>,
	after: Option<(DateTime<Utc>, Uuid)>,
	limit: u64,
) -> Result<Vec<(String, Uuid, DateTime<Utc>)>> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::DutyTicks);
	let rows: Vec<(String, Uuid, DateTime<Utc>)> = native::query_as(&due_query(now, after, limit))
		.columns(&["tenant", "id", "next_fire_at"])
		.fetch_all(pool.in_scope(&scope))
		.await?;
	Ok(rows)
}

/// The statement [`due`] runs.
fn due_query(now: DateTime<Utc>, after: Option<(DateTime<Utc>, Uuid)>, limit: u64) -> String {
	let mut query = Query::select();
	query
		.columns([
			Alias::new("tenant"),
			Alias::new("id"),
			Alias::new("next_fire_at"),
		])
		.from(Alias::new("duties"))
		.and_where(Expr::col("halt").is_null())
		.and_where(Expr::col("next_fire_at").lte(Expr::value(now)))
		// A deleted Tenant's Duties are stopped with it; never start one
		// in the moment between the deletion and that stop.
		.and_where(
			Expr::exists(
				Query::select()
					.expr(Expr::value(1_i32))
					.from(Alias::new("tenant_deletions"))
					.and_where(
						Expr::col((Alias::new("tenant_deletions"), Alias::new("tenant")))
							.equals((Alias::new("duties"), Alias::new("tenant"))),
					)
					.and_where(Expr::col("restored_at").is_null())
					.to_owned(),
			)
			.not(),
		);
	if let Some((at, id)) = after {
		query.cond_where(
			Condition::any()
				.add(Expr::col("next_fire_at").gt(Expr::value(at)))
				.add(
					Condition::all()
						.add(Expr::col("next_fire_at").eq(Expr::value(at)))
						.add(Expr::col("id").gt(Expr::value(id))),
				),
		);
	}
	query
		.order_by(Alias::new("next_fire_at"), Order::Asc)
		.order_by(Alias::new("id"), Order::Asc)
		.limit(limit)
		.to_string(PostgresQueryBuilder)
}

/// Claims a due Duty for this transaction, or None when another replica
/// holds it or already moved it on. The lock lasts until the transaction ends.
pub(crate) async fn claim(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
	now: DateTime<Utc>,
) -> Result<Option<Duty>> {
	let ids: Vec<Uuid> = native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("duties"))
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

/// What stops Duties besides a person.
#[derive(Clone, Copy)]
pub(crate) enum Stop<'a> {
	/// Their Requester left the Tenant or lost access.
	Requester(&'a str),
	/// A person stopped the flow whose Runs set them up (#1012); they pause,
	/// and only their Requester resumes them.
	Flow(&'a [Uuid]),
	/// Their fixed Thread was deleted.
	Thread(Uuid),
	/// The Tenant was deleted.
	Tenant,
}

/// Stops the Duties `stop` reaches and returns them. A Duty already
/// waiting for a successor, or stopped with the Tenant, keeps that reason, and
/// Tenant Deletion leaves every stopped Duty's reason as it was.
pub(crate) async fn stop(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	stop: Stop<'_>,
	now: DateTime<Utc>,
) -> Result<Vec<Duty>> {
	let query = DutyRecord::objects().filter(DutyRecord::field_tenant().eq(tenant.to_owned()));
	let (query, halt) = match stop {
		Stop::Requester(subject) => (
			query.filter(DutyRecord::field_requester().eq(subject.to_owned())),
			Halt::RequesterLeft,
		),
		Stop::Thread(thread) => (
			query.filter(DutyRecord::field_thread_id().eq(thread)),
			Halt::ThreadDeleted,
		),
		Stop::Tenant => (query, Halt::TenantDeleted),
		Stop::Flow(ids) => (
			query.filter(DutyRecord::field_id().is_in(ids.to_vec())),
			Halt::Paused,
		),
	};
	// Deleting a Thread holds its row, and a tick holds its Duty while it
	// reads that row: waiting here could deadlock. A Duty a tick holds is
	// skipped; that tick then finds the Thread deleted and stops it.
	let query = match stop {
		Stop::Thread(thread) => {
			let held: Vec<Uuid> = native::query_scalar(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("duties"))
					.and_where(Expr::col("tenant").eq(tenant.to_owned()))
					.and_where(Expr::col("thread_id").eq(Expr::value(thread)))
					.lock(LockType::Update)
					.lock_behavior(LockBehavior::SkipLocked)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_all(&mut *tx)
			.await?;
			if held.is_empty() {
				return Ok(vec![]);
			}
			query.filter(DutyRecord::field_id().is_in(held))
		}
		Stop::Requester(_) | Stop::Tenant | Stop::Flow(_) => query,
	};
	let rows = query
		.select_for_update()
		.all_with_executor(tx)
		.await
		.map_err(framework)?;
	let mut stopped = vec![];
	for row in rows {
		let mut duty = domain(row)?;
		let keep = match halt {
			// A paused Duty stays paused when its Thread goes; resuming
			// it then refuses until the Thread is changed.
			Halt::ThreadDeleted => duty.halt.is_some(),
			Halt::RequesterLeft => {
				matches!(duty.halt, Some(Halt::RequesterLeft | Halt::TenantDeleted))
			}
			// A Duty already stopped keeps its reason through a Tenant
			// Deletion and its restore, such as one waiting for a successor.
			_ => duty.halt.is_some(),
		};
		if keep {
			continue;
		}
		duty.stop(halt, now);
		duty.updated_at = now;
		stopped.push(save(tx, tenant, &duty).await?);
	}
	Ok(stopped)
}

/// Tasks Duties started in the last week, of the given Duties' ticks.
pub(crate) async fn recent_tasks(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duties: &[Uuid],
	since: DateTime<Utc>,
) -> Result<Vec<Uuid>> {
	if duties.is_empty() {
		return Ok(vec![]);
	}
	Ok(DutyTickRecord::objects()
		.filter(DutyTickRecord::field_tenant().eq(tenant.to_owned()))
		.filter(DutyTickRecord::field_duty_id().is_in(duties.to_vec()))
		.filter(DutyTickRecord::field_recorded_at().gte(since))
		.filter(DutyTickRecord::field_task_id().is_not_null())
		.filter(DutyTickRecord::field_started_by().is_null())
		.all_with_executor(tx)
		.await
		.map_err(framework)?
		.into_iter()
		.filter_map(|row| row.task_id)
		.collect())
}

/// The Duty an Agent's tool call with this invocation key registered.
pub(crate) async fn created_by(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	run: Uuid,
	key: &str,
) -> Result<Option<Duty>> {
	DutyRecord::objects()
		.filter(DutyRecord::field_tenant().eq(tenant.to_owned()))
		.filter(DutyRecord::field_created_by_run_id().eq(Some(run)))
		.filter(DutyRecord::field_creation_key().eq(Some(key.to_owned())))
		.all_with_executor(tx)
		.await
		.map_err(framework)?
		.pop()
		.map(domain)
		.transpose()
}

/// The long-lived credential of `subject`'s enabled Mapping in `tenant`.
pub(crate) async fn mapping_credential(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	subject: &str,
) -> Result<Option<Uuid>> {
	native::query_scalar::<Uuid>(
		&Query::select()
			.column(Alias::new("credential_id"))
			.from(Alias::new("dashboard_mappings"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("subject").eq(subject.to_owned()))
			.and_where(Expr::col("enabled").eq(true))
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut *tx)
	.await
}

#[cfg(test)]
mod tests;

/// Records the Duty a Task works for (#1012); see threads 0038.
pub(crate) async fn mark_task_duty(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	task: Uuid,
	duty: Uuid,
) -> Result<()> {
	native::query(
		&Query::update()
			.table(Alias::new("tasks"))
			.value_expr(Alias::new("duty_id"), Expr::value(duty))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("id").eq(Expr::value(task)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	Ok(())
}

/// The Node of the Thread a Duty works in.
pub(crate) async fn thread_node(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	thread: Uuid,
) -> Result<String> {
	native::query_scalar(
		&Query::select()
			.column(Alias::new("node"))
			.from(Alias::new("threads"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("id").eq(Expr::value(thread)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut *tx)
	.await
}
