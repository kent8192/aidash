//! Transactional Subnode lifecycle effects.
use crate::{Result as NativeResult, store::Store};
use aidash_application::{Result, ports::subnodes::EndScope};
use aidash_domain::{
	ControlSource, RunControl, RunControlAction,
	subnodes::{EndReason, SubnodeId},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::json;
use uuid::Uuid;

/// Fence a Thread effect against concurrent dissolution in the same
/// transaction as the effect. Non-Subnode Threads are unaffected.
pub(crate) async fn require_writable(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	thread: Uuid,
) -> NativeResult<()> {
	use reinhardt::db::orm::{QueryRow, execution::convert_values};
	let (sql, values) = Query::select()
		.column(Alias::new("state"))
		.from(Alias::new("subnodes"))
		.and_where(Expr::col("thread_id").eq(Expr::value(thread)))
		.lock(LockType::Share)
		.build(PostgresQueryBuilder);
	if let Some(row) = tx.fetch_optional(&sql, convert_values(values)).await? {
		let state: String = QueryRow::from_backend_row(row)
			.get("state")
			.ok_or(crate::Error::Forbidden)?;
		if state != "active" {
			return Err(crate::Error::Forbidden);
		}
	}
	Ok(())
}

/// Everything an end locks before it appends its first event: the creating
/// Run's row, all descendant Subnodes and their member Runs. A caller that appends
/// an event before it reaches `control_for_creator` calls this ahead of that
/// event; one that has not may rely on `control_for_creator`.
pub(crate) async fn lock_for_end(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	creator: Uuid,
) -> NativeResult<()> {
	lock_control(tx, creator).await?;
	aidash_application::subnodes::lock_end(&mut NativeCascade { tx }, creator).await?;
	Ok(())
}

async fn lock_control(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	run: Uuid,
) -> NativeResult<Option<(String, String)>> {
	crate::database::native::query_as(
		&Query::select()
			.columns(["control", "control_source"].map(Alias::new))
			.from(Alias::new("task_executions"))
			.and_where(Expr::col("id").eq(Expr::value(run)))
			.lock(LockType::NoKeyUpdate)
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["control", "control_source"])
	.fetch_optional(&mut *tx)
	.await
}

async fn safety_paused(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	run: Uuid,
) -> NativeResult<bool> {
	Ok(lock_control(tx, run)
		.await?
		.is_some_and(|(control, source)| {
			control == RunControl::Paused.as_str()
				&& source == ControlSource::DeploymentAdministrator.as_str()
		}))
}

/// A Tenant resume only propagates when it releases a Safety Stop pause.
pub(crate) async fn lock_for_resume(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	creator: Uuid,
) -> NativeResult<bool> {
	let cascade = safety_paused(tx, creator).await?;
	if cascade {
		aidash_application::subnodes::lock_end(&mut NativeCascade { tx }, creator).await?;
	}
	Ok(cascade)
}

pub(crate) struct Cascade<'a> {
	pub store: &'a Store,
	pub tx: &'a mut dyn reinhardt::db::backends::TransactionExecutor,
}
#[async_trait]
impl EndScope for Cascade<'_> {
	async fn active_subnodes(&mut self, creator: Uuid) -> Result<Vec<(SubnodeId, Uuid)>> {
		NativeCascade { tx: self.tx }.active_subnodes(creator).await
	}
	async fn mark_ended(&mut self, id: SubnodeId, thread: Uuid, reason: EndReason) -> Result<()> {
		let reason = serde_json::to_value(reason)?;
		let reason = reason.as_str().ok_or_else(|| {
			aidash_application::Error::Invalid("invalid Subnode end reason".into())
		})?;
		crate::database::native::query(
			&Query::update()
				.table(Alias::new("subnodes"))
				.value(Alias::new("state"), "dissolved")
				.value_expr(Alias::new("ended_at"), Expr::current_timestamp())
				.value(Alias::new("ended_reason"), reason)
				.and_where(Expr::col("id").eq(Expr::value(id.0)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *self.tx)
		.await?;
		self.store
			.event_in_executor(
				self.tx,
				Some(thread),
				"subnode.dissolved",
				json!({"subnode_id":id.0,"reason":reason}),
			)
			.await?;
		Ok(())
	}
	async fn member_runs(&mut self, thread: Uuid, resuming: bool) -> Result<Vec<(Uuid, bool)>> {
		NativeCascade { tx: self.tx }
			.member_runs(thread, resuming)
			.await
	}
	async fn control_run(
		&mut self,
		run: Uuid,
		action: RunControlAction,
		source: ControlSource,
	) -> Result<()> {
		// Preserve the control revision of an existing Safety Stop. A direct
		// stop's receipt identifies the paused branch during a later resume.
		let skip = match action {
			RunControlAction::Pause => safety_paused(self.tx, run).await?,
			RunControlAction::Resume => !safety_paused(self.tx, run).await?,
			RunControlAction::Cancel => false,
		};
		if skip {
			return Ok(());
		}
		self.store
			.control_one_in_as(self.tx, run, action, source)
			.await?;
		Ok(())
	}
}

/// The same cascade when a native compound transaction owns an erased
/// executor rather than Store's concrete transaction wrapper.
pub(crate) struct NativeCascade<'a> {
	pub tx: &'a mut dyn reinhardt::db::backends::TransactionExecutor,
}
#[async_trait]
impl EndScope for NativeCascade<'_> {
	async fn active_subnodes(&mut self, creator: Uuid) -> Result<Vec<(SubnodeId, Uuid)>> {
		use reinhardt::db::orm::{QueryRow, execution::convert_values};
		let (sql, values) = Query::select()
			.columns(["id", "thread_id"].map(Alias::new))
			.from(Alias::new("subnodes"))
			.and_where(Expr::col("creator_run_id").eq(Expr::value(creator)))
			.and_where(Expr::col("state").eq("active"))
			.order_by(Alias::new("id"), Order::Asc)
			.lock(LockType::Update)
			.build(PostgresQueryBuilder);
		let rows = self
			.tx
			.fetch_all(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?;
		rows.into_iter()
			.map(|row| {
				let row = QueryRow::from_backend_row(row);
				let id = row.get("id").ok_or(crate::Error::Forbidden)?;
				let thread = row.get("thread_id").ok_or(crate::Error::Forbidden)?;
				Ok((SubnodeId(id), thread))
			})
			.collect()
	}
	async fn mark_ended(&mut self, id: SubnodeId, thread: Uuid, reason: EndReason) -> Result<()> {
		use reinhardt::db::orm::execution::convert_values;
		let value = serde_json::to_value(reason)?;
		let reason = value.as_str().ok_or_else(|| {
			aidash_application::Error::Invalid("invalid Subnode end reason".into())
		})?;
		let (sql, values) = Query::update()
			.table(Alias::new("subnodes"))
			.value(Alias::new("state"), "dissolved")
			.value_expr(Alias::new("ended_at"), Expr::current_timestamp())
			.value(Alias::new("ended_reason"), reason)
			.and_where(Expr::col("id").eq(Expr::value(id.0)))
			.build(PostgresQueryBuilder);
		self.tx
			.execute(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?;
		crate::apps::execution::models::event_records::append(
			self.tx,
			Some(thread),
			"subnode.dissolved",
			json!({"subnode_id":id.0,"reason":reason}),
		)
		.await?;
		Ok(())
	}
	async fn member_runs(&mut self, thread: Uuid, resuming: bool) -> Result<Vec<(Uuid, bool)>> {
		let run = Alias::new("r");
		let controllable = Expr::col("phase")
			.is_not_in(["COMPLETED", "FAILED", "CANCELLED"])
			.and(Expr::col("control").ne("CANCELLED"));
		// Subnodes outlive a completed creator, so keep walking through ended
		// members when they still have active descendants of their own.
		let creates_subnodes = Expr::exists(
			Query::select()
				.expr(Expr::value(1_i32))
				.from(Alias::new("subnodes"))
				.and_where(Expr::col("creator_run_id").equals((run.clone(), Alias::new("id"))))
				.and_where(Expr::col("state").eq("active"))
				.to_owned(),
		);
		let mut query = Query::select();
		query
			.column(Alias::new("id"))
			.expr_as(controllable.clone(), Alias::new("controllable"))
			.from_as(Alias::new("task_executions"), run.clone())
			.and_where(Expr::col("thread_id").eq(Expr::value(thread)))
			.and_where(controllable.clone().or(creates_subnodes))
			.order_by(Alias::new("id"), Order::Asc)
			.lock(LockType::NoKeyUpdate);
		if resuming {
			// The receipt holds the revision immediately before the stop.
			let direct_pause = Expr::exists(
				Query::select()
					.expr(Expr::value(1_i32))
					.from(Alias::new("run_safety_stops"))
					.and_where(Expr::col("run_id").equals((run.clone(), Alias::new("id"))))
					.and_where(Expr::col("tenant").equals((run.clone(), Alias::new("tenant"))))
					.and_where(Expr::col("action").eq("pause"))
					.and_where(Expr::col("outcome").eq("accepted"))
					.and_where(
						Expr::col("expected_control_revision").eq(Expr::col((
							run,
							Alias::new("control_revision"),
						))
						.sub(1_i64)),
					)
					.to_owned(),
			);
			query.and_where(controllable.and(direct_pause).not());
		}
		Ok(
			crate::database::native::query_as(&query.to_string(PostgresQueryBuilder))
				.columns(&["id", "controllable"])
				.fetch_all(&mut *self.tx)
				.await?,
		)
	}
	async fn control_run(
		&mut self,
		run: Uuid,
		action: RunControlAction,
		source: ControlSource,
	) -> Result<()> {
		use reinhardt::db::orm::{QueryRow, execution::convert_values};
		// This is the cancellation-only branch of Store's control path. It is
		// needed for native multi-party transaction executors, and preserves
		// the same control revision, Safety Stop provenance, preview shutdown,
		// input-ledger fence and execution.control event.
		if action != RunControlAction::Cancel {
			return Err(aidash_application::Error::Invalid(
				"native terminal cascade only cancels".into(),
			));
		}
		crate::apps::execution::models::Run::worker_context(self.tx).await?;
		let (sql, values) = Query::update()
			.table(Alias::new("task_executions"))
			.value(Alias::new("control"), "CANCELLED")
			.value(Alias::new("control_source"), source.as_str())
			.value_expr(
				Alias::new("control_revision"),
				Expr::col("control_revision").add(Expr::value(1_i64)),
			)
			.value_expr(
				Alias::new("revision"),
				Expr::col("revision").add(Expr::value(1_i64)),
			)
			.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::col("id").eq(Expr::value(run)))
			.and_where(Expr::col("phase").is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
			.and_where(Expr::col("control").ne("CANCELLED"))
			.returning([Alias::new("thread_id")])
			.build(PostgresQueryBuilder);
		let row = self
			.tx
			.fetch_optional(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?
			.ok_or_else(|| {
				aidash_application::Error::Conflict("member Run is already terminal".into())
			})?;
		let thread: Uuid = QueryRow::from_backend_row(row)
			.get("thread_id")
			.ok_or(crate::Error::Forbidden)?;
		crate::apps::execution::models::HumanRequest::close_for_run(self.tx, run).await?;
		crate::apps::execution::repositories::live_previews::mark_stopped_for_run(
			self.tx,
			run,
			aidash_domain::preview::live::StopReason::RunCancelled,
		)
		.await?;
		crate::apps::execution::models::event_records::append_run(
			self.tx,
			run,
			Some(thread),
			"execution.control",
			json!({"run_id":run,"action":action,"source":source}),
		)
		.await?;
		Ok(())
	}
}
