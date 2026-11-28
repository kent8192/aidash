//! Task Goals and verdicts, under the Thread's authorization transaction.

use crate::{Error, Result, database::native};
use aidash_domain::{
	Run, RunInspection, RunState, Task, TaskStatus, ThinkingState,
	goal::{Edit, Verdict, VerdictRecord, validate_goal},
	run_state::RawRun,
};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "TaskGoalDetail")]
pub(crate) struct Detail {
	pub goal: Option<String>,
	pub edits: Vec<Edit>,
	pub verdicts: Vec<VerdictRecord>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "TaskGoalChange")]
pub(crate) struct Change {
	#[schemars(length(max = 4000))]
	pub goal: Option<String>,
	pub expected_revision: i64,
	#[serde(default)]
	pub resume: bool,
}

pub(crate) async fn locked_task(tx: &mut dyn TransactionExecutor, id: Uuid) -> Result<Task> {
	crate::database::query_as(
		&Query::select()
			.expr(Expr::col(ColumnRef::Asterisk))
			.from(Alias::new("tasks"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(tx)
	.await?
	.ok_or(Error::Forbidden)
}

pub(crate) async fn detail(tx: &mut dyn TransactionExecutor, task: &Task) -> Result<Detail> {
	let row = native::query(
		&Query::select()
			.columns(["goal_history", "goal_verdicts"].map(Alias::new))
			.from(Alias::new("tasks"))
			.and_where(Expr::col("id").eq(Expr::value(task.id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(tx)
	.await?;
	let read = |column| {
		row.try_get::<Option<Value>>(column)
			.map(|value| value.unwrap_or_else(|| serde_json::json!([])))
	};
	Ok(Detail {
		goal: task.goal.clone(),
		edits: serde_json::from_value(read("goal_history")?)?,
		verdicts: serde_json::from_value(read("goal_verdicts")?)?,
	})
}

pub(crate) async fn record(
	tx: &mut dyn TransactionExecutor,
	id: Uuid,
	verdict: &VerdictRecord,
) -> Result<VerdictRecord> {
	let task = locked_task(tx, id).await?;
	if task.goal.as_deref() != Some(&verdict.goal) || task.status != TaskStatus::Running {
		return Err(Error::Conflict(
			"Verdict belongs to another Goal or execution".into(),
		));
	}
	let mut detail = detail(tx, &task).await?;
	if let Some(existing) = detail.verdicts.iter().find(|existing| {
		existing.run_id == verdict.run_id
			&& existing.response_epoch == verdict.response_epoch
			&& existing.goal == verdict.goal
	}) {
		return Ok(existing.clone());
	}
	detail.verdicts.push(verdict.clone());
	native::query(
		&Query::update()
			.table(Alias::new("tasks"))
			.value_expr(
				Alias::new("goal_verdicts"),
				Expr::value(serde_json::to_value(detail.verdicts)?),
			)
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(tx)
	.await?;
	Ok(verdict.clone())
}

pub(crate) struct RemoteResume {
	pub grant_id: Uuid,
	pub message_id: Uuid,
}

pub(crate) async fn change(
	tx: &mut dyn TransactionExecutor,
	task: &Task,
	actor: &str,
	input: Change,
) -> Result<(Detail, Option<RemoteResume>)> {
	validate_goal(input.goal.as_deref())?;
	if task.created_by != actor {
		return Err(Error::Forbidden);
	}
	let retry = input.resume
		&& task.status == TaskStatus::Running
		&& task.goal.is_some()
		&& input.goal == task.goal
		&& task.revision.checked_sub(1) == Some(input.expected_revision);
	if !retry && task.revision != input.expected_revision {
		return Err(Error::Conflict("Task changed".into()));
	}
	let mut detail = detail(tx, task).await?;
	let mut remote = None;
	if input.resume {
		let latest = detail
			.verdicts
			.last()
			.filter(|record| record.judgment.verdict == Verdict::Impossible)
			.ok_or_else(|| Error::Conflict("Task has no impossible verdict".into()))?;
		if !retry
			&& (task.status != TaskStatus::Failed
				|| input.goal.is_none()
				|| input.goal == task.goal)
		{
			return Err(Error::Conflict(
				"Resuming requires a rewritten Goal on a failed Task".into(),
			));
		}
		if retry && task.goal.as_deref() == Some(&latest.goal) {
			return Err(Error::Conflict("Resuming requires a rewritten Goal".into()));
		}
		let message_id = last_completion_message(tx, task.id, task.thread_id).await?;
		if let Some(raw) = locked_execution(tx, latest.run_id).await? {
			if raw.metadata.task_id != task.id {
				return Err(Error::Forbidden);
			}
			resume(tx, raw.decode()?, message_id).await?;
		} else {
			let grant_id = native::query_scalar(
				&Query::select()
					.column(Alias::new("grant_id"))
					.from(Alias::new("authorization_remote_execution"))
					.and_where(Expr::col("task_id").eq(Expr::value(task.id)))
					.and_where(Expr::col("admission_id").eq(Expr::value(latest.run_id)))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut *tx)
			.await?
			.ok_or_else(|| Error::Conflict("Execution is unavailable".into()))?;
			remote = Some(RemoteResume {
				grant_id,
				message_id,
			});
		}
	} else if task.status.is_terminal() {
		return Err(Error::Conflict("Task is terminal".into()));
	}
	if input.goal != task.goal {
		detail.edits.push(Edit {
			goal: input.goal.clone(),
			edited_by: actor.to_owned(),
			edited_at: chrono::Utc::now(),
		});
		let mut update = Query::update();
		update
			.table(Alias::new("tasks"))
			.value_expr(Alias::new("goal"), Expr::value(input.goal.clone()))
			.value_expr(
				Alias::new("goal_history"),
				Expr::value(serde_json::to_value(&detail.edits)?),
			)
			.value_expr(Alias::new("revision"), Expr::col("revision").add(1))
			.and_where(Expr::col("id").eq(Expr::value(task.id)));
		if input.resume {
			update
				.value_expr(Alias::new("status"), Expr::value("RUNNING"))
				.value_expr(Alias::new("completion_key"), Expr::value(None::<String>));
		}
		native::query(&update.to_string(PostgresQueryBuilder))
			.execute(tx)
			.await?;
	}
	detail.goal = input.goal;
	Ok((detail, remote))
}

async fn last_completion_message(
	tx: &mut dyn TransactionExecutor,
	task: Uuid,
	thread: Uuid,
) -> Result<Uuid> {
	native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("messages"))
			.and_where(Expr::col("thread_id").eq(Expr::value(thread)))
			.and_where(Expr::cust_with_values(
				"(completion->>'task_id')::uuid = ?",
				[Expr::value(task)],
			))
			.order_by(Alias::new("created_at"), reinhardt::query::Order::Desc)
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(tx)
	.await
}

pub(crate) async fn locked_execution(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
) -> Result<Option<RawRun>> {
	crate::database::query_as(
		&Query::select()
			.expr(Expr::col(ColumnRef::Asterisk))
			.from(Alias::new("task_executions"))
			.and_where(Expr::col("id").eq(Expr::value(run)))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(tx)
	.await
}

pub(crate) async fn resume(
	tx: &mut dyn TransactionExecutor,
	mut run: Run,
	message_id: Uuid,
) -> Result<RunInspection> {
	if run.context.goal_resume_message == Some(message_id) {
		return Ok(run.into());
	}
	if !matches!(run.state, RunState::Failed(_))
		|| !run
			.context
			.final_completion
			.as_ref()
			.is_some_and(|completion| {
				completion.message_id == message_id
					&& completion
						.verdict
						.as_ref()
						.is_some_and(|verdict| verdict.verdict == Verdict::Impossible)
			}) {
		return Err(Error::Conflict("Execution has not settled".into()));
	}
	run.context.goal_resume_message = Some(message_id);
	run.context.final_completion = None;
	run.context.completion_attempt = run
		.context
		.completion_attempt
		.checked_add(1)
		.ok_or_else(|| Error::Conflict("Completion attempt limit reached".into()))?;
	run.context.goal_transcript = None;
	run.context.goal_judging = Default::default();
	run.state = RunState::Thinking(ThinkingState::default());
	let next = run.step.saturating_add(1);
	let changed = native::query(
		&Query::update()
			.table(Alias::new("task_executions"))
			.value_expr(Alias::new("phase"), Expr::value("THINKING"))
			.value_expr(Alias::new("step"), Expr::value(next))
			.value_expr(Alias::new("pending"), Expr::value(run.stored_pending()?))
			.value_expr(
				Alias::new("context"),
				Expr::value(serde_json::to_value(&run.context)?),
			)
			.value_expr(Alias::new("control"), Expr::value("ACTIVE"))
			.value_expr(Alias::new("error"), Expr::value(None::<String>))
			.value_expr(Alias::new("revision"), Expr::col("revision").add(1))
			.and_where(Expr::col("id").eq(Expr::value(run.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(tx)
	.await?;
	if changed.rows_affected() != 1 {
		return Err(Error::Conflict("Execution changed".into()));
	}
	run.step = next;
	run.revision += 1;
	run.control = aidash_domain::RunControl::Active;
	run.error = None;
	Ok(run.into())
}
