//! Router and Watcher scripts of a Duty (#1011, ADR 0142): their saved
//! versions, the Tasks a Duty starts to run them without a model, and the
//! Duties whose Watcher is due. Every table here is keyed on the Duty or the
//! Task and goes with the Duty.
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::{
	TaskStatus,
	duties::{
		Definition, Duty,
		scripts::{Invocation, Language, OpenTask, Role, Script, TaskScript, Version},
	},
};
use chrono::{DateTime, Utc};
use reinhardt::db::{backends::TransactionExecutor, orm::Model};
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait, JoinType, LockBehavior, LockType, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder, TableRef,
};
use serde_json::Value;
use uuid::Uuid;

use crate::apps::workspaces::models::{DutyScriptRunRecord, DutyScriptVersionRecord};
use crate::database::native::{self, Pool};
use crate::{Error, Result};

fn framework(error: impl Into<reinhardt::core::exception::Error>) -> Error {
	Error::from(error.into())
}

/// Task statuses of a Task still under way.
const OPEN: [&str; 4] = ["OPEN", "CLAIMED", "RUNNING", "BLOCKED"];

fn version_of(row: DutyScriptVersionRecord) -> Result<Version> {
	Ok(Version {
		id: row.id,
		duty_id: row.duty_id,
		role: Role::parse(&row.role)?,
		script: Script {
			language: Language::parse(&row.language)?,
			source: row.source,
		},
		every_seconds: row.every_seconds,
		author: row.author,
		created_at: row.created_at,
	})
}

async fn insert_version(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: Uuid,
	(role, script, every_seconds): (Role, &Script, Option<i64>),
	(author, now): (&str, DateTime<Utc>),
) -> Result<()> {
	let row = DutyScriptVersionRecord::build()
		.id(Uuid::now_v7())
		.tenant(tenant.to_owned())
		.duty_id(duty)
		.role(role.as_str().to_owned())
		.language(script.language.as_str().to_owned())
		.source(script.source.clone())
		.every_seconds(every_seconds)
		.author(author.to_owned())
		.created_at(now)
		.finish();
	DutyScriptVersionRecord::objects()
		.insert_with_executor(tx, &row)
		.await
		.map_err(framework)?;
	Ok(())
}

/// Keeps every script `duty` now has that `before` did not, as a version saved
/// by `author` (#1011): a new Router, or a Watcher whose script or interval
/// changed. A removal saves no version.
pub(crate) async fn save_versions(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	before: Option<&Definition>,
	duty: &Duty,
	(author, now): (&str, DateTime<Utc>),
) -> Result<()> {
	let definition = &duty.definition;
	if let Some(router) = &definition.router
		&& before.and_then(|before| before.router.as_ref()) != Some(router)
	{
		insert_version(
			tx,
			tenant,
			duty.id,
			(Role::Router, router, None),
			(author, now),
		)
		.await?;
	}
	if let Some(watcher) = &definition.watcher
		&& before.and_then(|before| before.watcher.as_ref()) != Some(watcher)
	{
		insert_version(
			tx,
			tenant,
			duty.id,
			(Role::Watcher, &watcher.script, Some(watcher.every_seconds)),
			(author, now),
		)
		.await?;
	}
	Ok(())
}

/// The latest saved script versions of a Duty, newest first.
pub(crate) async fn versions(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: Uuid,
	limit: u32,
) -> Result<Vec<Version>> {
	DutyScriptVersionRecord::objects()
		.filter(DutyScriptVersionRecord::field_tenant().eq(tenant.to_owned()))
		.filter(DutyScriptVersionRecord::field_duty_id().eq(duty))
		.order_by(&["-created_at", "-id"])
		.limit(limit as usize)
		.all_with_executor(tx)
		.await
		.map_err(framework)?
		.into_iter()
		.map(version_of)
		.collect()
}

/// What a script Task records when the Duty starts it.
pub(crate) struct NewRun<'a> {
	pub task: Uuid,
	pub duty: Uuid,
	pub role: Role,
	/// The event a Router decides about.
	pub event: Option<Uuid>,
	pub invocation: &'a Invocation,
	pub now: DateTime<Utc>,
}

pub(crate) async fn insert_run(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	run: NewRun<'_>,
) -> Result<()> {
	// The Task keeps that it runs a script after the row goes with its Duty,
	// so its Run fails instead of asking a model (see workspaces 0062).
	native::query(
		&Query::update()
			.table(Alias::new("tasks"))
			.value(Alias::new("duty_script"), true)
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("id").eq(Expr::value(run.task)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	let row = DutyScriptRunRecord::build()
		.task_id(run.task)
		.tenant(tenant.to_owned())
		.duty_id(run.duty)
		.role(run.role.as_str().to_owned())
		.event_id(run.event)
		.language(run.invocation.language.as_str().to_owned())
		.source(run.invocation.source.clone())
		.stdin(run.invocation.stdin.clone())
		.created_at(run.now)
		.settled_at(None)
		.finish();
	DutyScriptRunRecord::objects()
		.insert_with_executor(tx, &row)
		.await
		.map_err(framework)?;
	Ok(())
}

/// A Task's script marker beside its script row, if the row is still there:
/// language, source and stdin.
type TaskScriptRow = (bool, Option<String>, Option<String>, Option<String>);

/// What the Run of `task` executes instead of a model: the script a Duty
/// started that Task to run, or `Gone` when the script went with its Duty
/// (#1011). Read by the harness on each step of a Run, in the Run's own scope.
pub(crate) async fn invocation(tx: &mut dyn TransactionExecutor, task: Uuid) -> Result<TaskScript> {
	let (tasks, runs) = (Alias::new("tasks"), Alias::new("duty_script_runs"));
	let row: Option<TaskScriptRow> = native::query_as(
		&Query::select()
			.column((tasks.clone(), Alias::new("duty_script")))
			.column((runs.clone(), Alias::new("language")))
			.column((runs.clone(), Alias::new("source")))
			.column((runs.clone(), Alias::new("stdin")))
			.from(tasks.clone())
			.join(
				JoinType::LeftJoin,
				runs.clone(),
				Condition::all()
					.add(
						Expr::col((runs.clone(), Alias::new("task_id")))
							.equals((tasks.clone(), Alias::new("id"))),
					)
					.add(
						Expr::col((runs, Alias::new("tenant")))
							.equals((tasks.clone(), Alias::new("tenant"))),
					),
			)
			.and_where(Expr::col((tasks, Alias::new("id"))).eq(Expr::value(task)))
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["duty_script", "language", "source", "stdin"])
	.fetch_optional(&mut *tx)
	.await?;
	Ok(match row {
		Some((_, Some(language), Some(source), Some(stdin))) => TaskScript::Script(Invocation {
			language: Language::parse(&language)?,
			source,
			stdin,
		}),
		Some((true, ..)) => TaskScript::Gone,
		_ => TaskScript::Model,
	})
}

/// Cancels the Runs of the Duty's script Tasks that have not settled, before
/// the Duty and its script rows go (#1011): a Router or Watcher Task never
/// becomes a model's Run on its stub description.
pub(crate) async fn cancel_open(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: Uuid,
) -> Result<()> {
	native::query(
		&Query::update()
			.table(Alias::new("task_executions"))
			.value(Alias::new("control"), "CANCELLED")
			.value(
				Alias::new("control_source"),
				aidash_domain::entities::ControlSource::System.as_str(),
			)
			.value_expr(
				Alias::new("control_revision"),
				Expr::col("control_revision").add(Expr::value(1_i64)),
			)
			.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(
				Expr::col("task_id").in_subquery(
					Query::select()
						.column(Alias::new("task_id"))
						.from(Alias::new("duty_script_runs"))
						.and_where(Expr::col("tenant").eq(tenant.to_owned()))
						.and_where(Expr::col("duty_id").eq(Expr::value(duty)))
						.and_where(Expr::col("settled_at").is_null())
						.to_owned(),
				),
			)
			.and_where(Expr::col("phase").is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
			.and_where(Expr::col("control").ne("CANCELLED"))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	Ok(())
}

/// Whether the Duty's previous Watcher run is still under way.
pub(crate) async fn watching(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: Uuid,
) -> Result<bool> {
	let runs = Alias::new("duty_script_runs");
	let open: Option<Uuid> = native::query_scalar(
		&Query::select()
			.column((runs.clone(), Alias::new("task_id")))
			.from(runs.clone())
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(Alias::new("tasks"), Alias::new("t")),
				Condition::all()
					.add(
						Expr::col((Alias::new("t"), Alias::new("id")))
							.equals((runs.clone(), Alias::new("task_id"))),
					)
					.add(Expr::col((Alias::new("t"), Alias::new("status"))).is_in(OPEN)),
			)
			.and_where(Expr::col((runs.clone(), Alias::new("tenant"))).eq(tenant.to_owned()))
			.and_where(Expr::col((runs.clone(), Alias::new("duty_id"))).eq(Expr::value(duty)))
			.and_where(Expr::col((runs.clone(), Alias::new("role"))).eq(Role::Watcher.as_str()))
			.and_where(Expr::col((runs, Alias::new("settled_at"))).is_null())
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut *tx)
	.await?;
	Ok(open.is_some())
}

/// A script Task whose Task ended and whose outcome the Duty has not applied.
#[derive(Debug, Clone)]
pub(crate) struct Ended {
	pub tenant: String,
	pub task: Uuid,
	pub duty: Uuid,
	pub role: Role,
	pub event: Option<Uuid>,
	pub status: TaskStatus,
}

/// Script Tasks that ended and are not yet settled, across Tenants, oldest first.
pub(crate) async fn ended(pool: &Pool, limit: u64) -> Result<Vec<Ended>> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::DutyTicks);
	let runs = Alias::new("duty_script_runs");
	let rows: Vec<(String, Uuid, Uuid, String, Option<Uuid>, String)> = native::query_as(
		&Query::select()
			.column((runs.clone(), Alias::new("tenant")))
			.column((runs.clone(), Alias::new("task_id")))
			.column((runs.clone(), Alias::new("duty_id")))
			.column((runs.clone(), Alias::new("role")))
			.column((runs.clone(), Alias::new("event_id")))
			.column((Alias::new("t"), Alias::new("status")))
			.from(runs.clone())
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(Alias::new("tasks"), Alias::new("t")),
				Condition::all()
					.add(
						Expr::col((Alias::new("t"), Alias::new("tenant")))
							.equals((runs.clone(), Alias::new("tenant"))),
					)
					.add(
						Expr::col((Alias::new("t"), Alias::new("id")))
							.equals((runs.clone(), Alias::new("task_id"))),
					),
			)
			.and_where(Expr::col((runs.clone(), Alias::new("settled_at"))).is_null())
			.and_where(Expr::col((Alias::new("t"), Alias::new("status"))).is_not_in(OPEN))
			.order_by((runs, Alias::new("created_at")), Order::Asc)
			.limit(limit)
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["tenant", "task_id", "duty_id", "role", "event_id", "status"])
	.fetch_all(pool.in_scope(&scope))
	.await?;
	rows.into_iter()
		.map(|(tenant, task, duty, role, event, status)| {
			Ok(Ended {
				tenant,
				task,
				duty,
				role: Role::parse(&role)?,
				event,
				status: serde_json::from_value(Value::String(status))?,
			})
		})
		.collect()
}

/// Marks a script Task settled. Whether it was still unsettled: another
/// replica may have settled it first, and the row lock makes the second wait.
pub(crate) async fn settle(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	task: Uuid,
	now: DateTime<Utc>,
) -> Result<bool> {
	let Some(mut row) = DutyScriptRunRecord::objects()
		.filter(DutyScriptRunRecord::field_tenant().eq(tenant.to_owned()))
		.filter(DutyScriptRunRecord::field_task_id().eq(task))
		.select_for_update()
		.all_with_executor(&mut *tx)
		.await
		.map_err(framework)?
		.pop()
	else {
		return Ok(false);
	};
	if row.settled_at.is_some() {
		return Ok(false);
	}
	row.settled_at = Some(now);
	DutyScriptRunRecord::objects()
		.save_with_executor(tx, &row)
		.await
		.map_err(framework)?;
	Ok(true)
}

/// What a script Task left behind: the text of its result, or why its Run
/// failed.
pub(crate) async fn outcome(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	task: Uuid,
) -> Result<(Option<String>, Option<String>)> {
	let content: Option<Value> = native::query_scalar(
		&Query::select()
			.column(Alias::new("content"))
			.from(Alias::new("artifacts"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("task_id").eq(Expr::value(task)))
			.order_by(Alias::new("created_at"), Order::Desc)
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut *tx)
	.await?;
	let error: Option<Option<String>> = native::query_scalar(
		&Query::select()
			.column(Alias::new("error"))
			.from(Alias::new("task_executions"))
			.and_where(Expr::col("task_id").eq(Expr::value(task)))
			.order_by(Alias::new("updated_at"), Order::Desc)
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut *tx)
	.await?;
	Ok((
		content.and_then(|content| content.as_str().map(str::to_owned)),
		error.flatten(),
	))
}

/// The open model Tasks of a Duty, as its Router sees them: Tasks its ticks
/// and events started that have not ended. Script Tasks are never among them.
pub(crate) async fn open_tasks(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: Uuid,
) -> Result<Vec<OpenTask>> {
	let ticks = Alias::new("duty_ticks");
	let rows: Vec<(Uuid, String)> = native::query_as(
		&Query::select()
			.column((Alias::new("t"), Alias::new("id")))
			.column((Alias::new("t"), Alias::new("title")))
			.from(ticks.clone())
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(Alias::new("tasks"), Alias::new("t")),
				Condition::all()
					.add(
						Expr::col((Alias::new("t"), Alias::new("tenant")))
							.equals((ticks.clone(), Alias::new("tenant"))),
					)
					.add(
						Expr::col((Alias::new("t"), Alias::new("id")))
							.equals((ticks.clone(), Alias::new("task_id"))),
					),
			)
			.and_where(Expr::col((ticks.clone(), Alias::new("tenant"))).eq(tenant.to_owned()))
			.and_where(Expr::col((ticks.clone(), Alias::new("duty_id"))).eq(Expr::value(duty)))
			.and_where(Expr::col((ticks.clone(), Alias::new("status"))).eq("started"))
			.and_where(Expr::col((Alias::new("t"), Alias::new("status"))).is_in(OPEN))
			.order_by((ticks, Alias::new("recorded_at")), Order::Asc)
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["id", "title"])
	.fetch_all(&mut *tx)
	.await?;
	Ok(rows
		.into_iter()
		.map(|(task_id, title)| OpenTask { task_id, title })
		.collect())
}

/// The Run working on `task` now, if it has not ended.
pub(crate) async fn active_run(
	tx: &mut dyn TransactionExecutor,
	task: Uuid,
) -> Result<Option<Uuid>> {
	native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("task_executions"))
			.and_where(Expr::col("task_id").eq(Expr::value(task)))
			.and_where(Expr::col("phase").is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
			.order_by(Alias::new("updated_at"), Order::Desc)
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut *tx)
	.await
}

/// Duties whose Watcher is due, across Tenants, oldest first.
pub(crate) async fn due_watchers(
	pool: &Pool,
	now: DateTime<Utc>,
	limit: u64,
) -> Result<Vec<(String, Uuid)>> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::DutyTicks);
	native::query_as(
		&Query::select()
			.columns([Alias::new("tenant"), Alias::new("id")])
			.from(Alias::new("duties"))
			.and_where(Expr::col("halt").is_null())
			.and_where(Expr::col("watcher").is_not_null())
			.and_where(Expr::col("watcher_next_at").lte(Expr::value(now)))
			.order_by(Alias::new("watcher_next_at"), Order::Asc)
			.limit(limit)
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["tenant", "id"])
	.fetch_all(pool.in_scope(&scope))
	.await
}

/// Locks a Duty whose Watcher is due, unless another replica holds it.
pub(crate) async fn claim_watcher(
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
			.and_where(Expr::col("watcher").is_not_null())
			.and_where(Expr::col("watcher_next_at").lte(Expr::value(now)))
			.lock(LockType::Update)
			.lock_behavior(LockBehavior::SkipLocked)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&mut *tx)
	.await?;
	if ids.is_empty() {
		return Ok(None);
	}
	super::duties::load(tx, tenant, id, false).await
}

/// Moves a locked Duty's Watcher to its next run. Only the Watcher's clock
/// changes, so the Duty's revision stays: every writer of a Duty locks and
/// reloads it first.
pub(crate) async fn advance_watcher(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: Uuid,
	next: Option<DateTime<Utc>>,
) -> Result<()> {
	native::query(
		&Query::update()
			.table(Alias::new("duties"))
			.value(Alias::new("watcher_next_at"), next)
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("id").eq(Expr::value(duty)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	Ok(())
}
