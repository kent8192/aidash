//! Native transactions retain eligibility locks, DB time, CAS fences and repair events.
use crate::apps::execution::{
	models::Run as RunRecord, repositories::store::run_state::run_unblocked,
};
use crate::{Error, Result};
use aidash_application::ports::activation::{Approval, RecoveryCursor, SchedulingScope};
use aidash_domain::{Run, RunMetadata, RunState, TaskStatus, run_state::RawRun};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::{QueryValue, Row, TransactionExecutor};
use reinhardt::db::orm::{QueryRow, execution::convert_values};
use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait, IntoIden, LockBehavior, LockType, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use serde_json::json;
use uuid::Uuid;

pub(crate) trait Executor: Send {
	fn executor(&mut self) -> &mut dyn TransactionExecutor;
}
impl Executor for Box<dyn TransactionExecutor> {
	fn executor(&mut self) -> &mut dyn TransactionExecutor {
		self.as_mut()
	}
}
impl Executor for &mut dyn TransactionExecutor {
	fn executor(&mut self) -> &mut dyn TransactionExecutor {
		*self
	}
}
pub(crate) struct Scope<E> {
	pub tx: E,
}

pub(crate) fn raw(row: Row) -> Result<RawRun> {
	let native: RunRecord = serde_json::from_value(QueryRow::from_backend_row(row).data)?;
	let mut data = serde_json::to_value(native)?;
	Ok(RawRun {
		metadata: serde_json::from_value(data.clone())?,
		context: data["context"].take(),
		pending: data["pending"].take(),
	})
}

fn approval(row: Row) -> Result<Approval> {
	Ok(Approval {
		state: row.get("state").map_err(FrameworkError::from)?,
		expires_at: if matches!(row.data.get("expires_at"), Some(QueryValue::Null)) {
			None
		} else {
			Some(row.get("expires_at").map_err(FrameworkError::from)?)
		},
	})
}

#[async_trait]
impl<E: Executor> SchedulingScope for Scope<E> {
	async fn now(&mut self) -> aidash_application::Result<DateTime<Utc>> {
		let (sql, values) = Query::select()
			.expr_as(Expr::current_timestamp(), Alias::new("now"))
			.build(PostgresQueryBuilder);
		self.tx
			.executor()
			.fetch_one(&sql, convert_values(values))
			.await
			.map_err(Error::from)?
			.get("now")
			.map_err(FrameworkError::from)
			.map_err(Error::from)
			.map_err(Into::into)
	}
	async fn candidates(
		&mut self,
		target: Option<Uuid>,
		cursor: RecoveryCursor,
	) -> aidash_application::Result<Vec<RawRun>> {
		let mut ready = Query::select();
		ready
			.column(ColumnRef::Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col("phase").is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
			.and_where(Expr::col("control").ne(Expr::value("PAUSED")))
			.and_where(Expr::col("revision").lt(Expr::value(i64::MAX - 2)))
			.cond_where(
				Condition::any()
					.add(Expr::col("lease_until").is_null())
					.add(Expr::col("lease_until").lte(Expr::current_timestamp())),
			)
			.and_where(run_unblocked())
			.order_by(Alias::new("updated_at"), Order::Asc)
			.order_by(Alias::new("id"), Order::Asc)
			.limit(128)
			.lock(LockType::Update)
			.lock_behavior(LockBehavior::SkipLocked);
		if let Some(id) = target {
			ready.and_where(Expr::col("id").eq(Expr::value(id)));
		}
		if let Some((at, id)) = cursor {
			ready.cond_where(
				Condition::any()
					.add(Expr::col("updated_at").gt(Expr::value(at)))
					.add(
						Condition::all()
							.add(Expr::col("updated_at").eq(Expr::value(at)))
							.add(Expr::col("id").gt(Expr::value(id))),
					),
			);
		}
		let (sql, values) = ready.build(PostgresQueryBuilder);
		self.tx
			.executor()
			.fetch_all(&sql, convert_values(values))
			.await
			.map_err(Error::from)?
			.into_iter()
			.map(raw)
			.collect::<Result<_>>()
			.map_err(Into::into)
	}
	async fn unblocked(&mut self, id: Uuid) -> aidash_application::Result<bool> {
		let candidate = Query::select()
			.expr(Expr::value(1_i64))
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(run_unblocked())
			.to_owned();
		exists(self.tx.executor(), candidate)
			.await
			.map_err(Into::into)
	}
	async fn human_answered(&mut self, id: Uuid) -> aidash_application::Result<bool> {
		let answered = Query::select()
			.expr(Expr::value(1_i64))
			.from(Alias::new("human_requests"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("response").is_not_null())
			.to_owned();
		exists(self.tx.executor(), answered)
			.await
			.map_err(Into::into)
	}
	async fn approval(&mut self, id: Uuid) -> aidash_application::Result<Option<Approval>> {
		let (sql, values) = Query::select()
			.columns([Alias::new("state"), Alias::new("expires_at")])
			.from(Alias::new("core_records"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.build(PostgresQueryBuilder);
		self.tx
			.executor()
			.fetch_optional(&sql, convert_values(values))
			.await
			.map_err(Error::from)?
			.map(approval)
			.transpose()
			.map_err(|e: Error| e.into())
	}
	async fn dependencies_ready(&mut self, task: Uuid) -> aidash_application::Result<bool> {
		let mut dependencies = Query::select()
			.expr(Expr::value(1_i64))
			.from(Alias::new("task_dependencies"))
			.inner_join(
				Alias::new("tasks"),
				Expr::col((Alias::new("tasks"), Alias::new("id")))
					.equals((Alias::new("task_dependencies"), Alias::new("dependency_id"))),
			)
			.and_where(
				Expr::col((Alias::new("task_dependencies"), Alias::new("task_id")))
					.eq(Expr::value(task)),
			)
			.to_owned();
		let mut incomplete = dependencies.clone();
		incomplete.and_where(
			Expr::col((Alias::new("tasks"), Alias::new("status")))
				.ne(Expr::value(TaskStatus::Completed.as_str())),
		);
		dependencies.and_where(
			Expr::col((Alias::new("tasks"), Alias::new("status"))).is_in([
				TaskStatus::Failed.as_str(),
				TaskStatus::Cancelled.as_str(),
				TaskStatus::Abandoned.as_str(),
			]),
		);
		let (sql, values) = Query::select()
			.expr_as(
				Expr::exists(incomplete)
					.not()
					.or(Expr::exists(dependencies)),
				Alias::new("ready"),
			)
			.build(PostgresQueryBuilder);
		self.tx
			.executor()
			.fetch_one(&sql, convert_values(values))
			.await
			.map_err(Error::from)?
			.get("ready")
			.map_err(FrameworkError::from)
			.map_err(Error::from)
			.map_err(Into::into)
	}
	async fn lease(
		&mut self,
		run: &Run,
		token: Uuid,
		seconds: i32,
	) -> aidash_application::Result<Option<Run>> {
		let (sql, values) = Query::update()
			.table(Alias::new("runs"))
			.value(Alias::new("pending"), run.stored_pending()?)
			.value(Alias::new("lease_owner"), token)
			.value_expr(
				Alias::new("lease_until"),
				crate::database::lease_deadline(seconds),
			)
			.value_expr(
				Alias::new("revision"),
				Expr::col("revision").add(Expr::value(1_i64)),
			)
			.value(Alias::new("ledger_worker_ready"), true)
			.and_where(Expr::col("id").eq(Expr::value(run.id)))
			.and_where(Expr::col("revision").eq(Expr::value(run.revision)))
			.and_where(worker_context())
			.returning_all()
			.build(PostgresQueryBuilder);
		self.tx
			.executor()
			.fetch_optional(&sql, convert_values(values))
			.await
			.map_err(Error::from)?
			.map(|row| raw(row)?.decode().map_err(Into::into))
			.transpose()
			.map_err(|e: Error| e.into())
	}
	async fn repair_lease(
		&mut self,
		run: &RunMetadata,
		token: Uuid,
		seconds: i32,
	) -> aidash_application::Result<Option<RunMetadata>> {
		let (sql, values) = Query::update()
			.table(Alias::new("runs"))
			.value(Alias::new("lease_owner"), token)
			.value_expr(
				Alias::new("lease_until"),
				crate::database::lease_deadline(seconds),
			)
			.value_expr(
				Alias::new("revision"),
				Expr::col("revision").add(Expr::value(1_i64)),
			)
			.value(Alias::new("ledger_worker_ready"), true)
			.and_where(Expr::col("id").eq(Expr::value(run.id)))
			.and_where(Expr::col("revision").eq(Expr::value(run.revision)))
			.and_where(Expr::col("phase").is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
			.and_where(Expr::col("control").ne(Expr::value("PAUSED")))
			.cond_where(
				Condition::any()
					.add(Expr::col("lease_until").is_null())
					.add(Expr::col("lease_until").lte(Expr::current_timestamp())),
			)
			.and_where(worker_context())
			.returning_all()
			.build(PostgresQueryBuilder);
		self.tx
			.executor()
			.fetch_optional(&sql, convert_values(values))
			.await
			.map_err(Error::from)?
			.map(|row| raw(row).map(|run| run.metadata))
			.transpose()
			.map_err(Into::into)
	}
	async fn invalid_state(
		&mut self,
		owned: &RunMetadata,
		token: Uuid,
		state: &RunState,
		reason: &str,
		node: &str,
	) -> aidash_application::Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new("runs"))
			.value(Alias::new("phase"), "WAITING")
			.value(
				Alias::new("pending"),
				aidash_domain::run_state::encode(state, &Default::default())?,
			)
			.value(Alias::new("error"), reason)
			.value_expr(
				Alias::new("revision"),
				Expr::col("revision").add(Expr::value(1_i64)),
			)
			.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
			.value(Alias::new("lease_owner"), None::<Uuid>)
			.value(Alias::new("lease_until"), None::<DateTime<Utc>>)
			.and_where(Expr::col("id").eq(Expr::value(owned.id)))
			.and_where(Expr::col("revision").eq(Expr::value(owned.revision)))
			.and_where(Expr::col("lease_owner").eq(Expr::value(token)))
			.and_where(Expr::col("lease_until").gt(Expr::current_timestamp()))
			.build(PostgresQueryBuilder);
		self.tx
			.executor()
			.execute(&sql, convert_values(values))
			.await
			.map_err(Error::from)?;
		// Preserve the original trigger/event transaction; damaged Context is never read.
		let (sql, values) = Query::insert()
			.into_table(Alias::new("events"))
			.columns([
				Alias::new("id"),
				Alias::new("node_id"),
				Alias::new("workspace_id"),
				Alias::new("kind"),
				Alias::new("data"),
			])
			.from_subquery(
				Query::select()
					.expr(Expr::value(Uuid::new_v4()))
					.expr(Expr::value(node))
					.expr(Expr::value(
						(node == owned.home_node).then_some(owned.workspace_id),
					))
					.expr(Expr::value("run.invalid_state"))
					.expr(Expr::value(
						json!({"run_id":owned.id,"task_id":owned.task_id,"category":reason}),
					))
					.to_owned(),
			)
			.build(PostgresQueryBuilder);
		self.tx
			.executor()
			.execute(&sql, convert_values(values))
			.await
			.map_err(Error::from)?;
		Ok(())
	}
}

async fn exists(
	tx: &mut dyn TransactionExecutor,
	select: reinhardt::query::SelectStatement,
) -> Result<bool> {
	let (sql, values) = Query::select()
		.expr_as(Expr::exists(select), Alias::new("found"))
		.build(PostgresQueryBuilder);
	Ok(tx
		.fetch_one(&sql, convert_values(values))
		.await?
		.get("found")
		.map_err(FrameworkError::from)?)
}

fn worker_context() -> reinhardt::query::SimpleExpr {
	SimpleExpr::FunctionCall(
		"set_config".into_iden(),
		vec![
			Expr::value("aidash.input_ledger_worker").into(),
			Expr::value("true").into(),
			Expr::value(true).into(),
		],
	)
	.eq(Expr::value("true"))
}

#[cfg(test)]
#[path = "scheduling/tests.rs"]
mod tests;
