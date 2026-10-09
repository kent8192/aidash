//! Inference Attempts, their per-Run progress sequence and outcome markers.
//!
//! Writers run inside a caller-owned transaction that already holds the Run row
//! lock, so `progress_seq` allocation is serialized per Run without the event
//! journal's global advisory lock. Only the low-rate outcome markers are
//! journal events.
use crate::apps::execution::models::{
	event_records,
	inference::{InferenceAttempt, InferenceProgressRecord},
};
use crate::database::native::{self, Decode, Executor};
use crate::{Error, Result};
use aidash_domain::provider::progress::{
	InferenceAttemptId, InferenceProgress, InterruptionReason, MAX_PROGRESS_ITEM_BYTES,
	ProgressOutcome, RETENTION, STARTED_EVENT,
};
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{Model, execution::convert_values};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, Func, IntoIden, JoinType, LockBehavior, LockType, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) const STARTED: &str = "started";
pub(crate) const TEXT: &str = "text";
pub(crate) const TOOL_CALL: &str = "tool_call";
pub(crate) const OUTCOME: &str = "outcome";

/// Attempts pruned per maintenance pass.
const PRUNE_BATCH: u64 = 64;
/// The database renders stored JSON with separators the compact encoder omits.
const STORAGE_MARGIN: usize = 64;

/// Durable identity of the Run that owns an attempt, read under its row lock.
pub(crate) struct RunTarget {
	pub id: Uuid,
	pub task_id: Uuid,
	pub workspace_id: Uuid,
	pub home_node: String,
	pub control: String,
}
crate::native_record!(RunTarget {
	id,
	task_id,
	workspace_id,
	home_node,
	control
});

impl RunTarget {
	/// Interrupted reason for an attempt whose worker no longer owns the Run.
	fn orphan_outcome(&self) -> ProgressOutcome {
		ProgressOutcome::Interrupted(if self.control == "CANCELLED" {
			InterruptionReason::Cancelled
		} else {
			InterruptionReason::LeaseLost
		})
	}
}

/// A stored progress row with its attempt's outcome, as delivered to streams.
pub(crate) struct ProgressRow {
	pub seq: i64,
	pub attempt_id: Uuid,
	pub kind: String,
	pub item: Value,
	pub created_at: DateTime<Utc>,
	pub outcome: Option<String>,
}
crate::native_record!(ProgressRow {
	seq,
	attempt_id,
	kind,
	item,
	created_at,
	outcome
});

struct PendingAttempt {
	id: Uuid,
}
crate::native_record!(PendingAttempt { id });

struct NextSeq {
	next: i64,
}
crate::native_record!(NextSeq { next });

async fn rows<T: Decode>(
	executor: impl Executor,
	statement: &impl QueryStatementBuilder,
) -> Result<Vec<T>> {
	let (sql, values) = statement.build(PostgresQueryBuilder);
	executor
		.rows(&sql, convert_values(values))
		.await?
		.iter()
		.map(|row| T::decode(row, &[]))
		.collect()
}

async fn execute(
	tx: &mut dyn TransactionExecutor,
	statement: &impl QueryStatementBuilder,
) -> Result<u64> {
	let (sql, values) = statement.build(PostgresQueryBuilder);
	Ok(tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected)
}

fn outcome_columns(outcome: ProgressOutcome) -> (&'static str, Option<&'static str>) {
	match outcome {
		ProgressOutcome::Accepted => ("accepted", None),
		ProgressOutcome::Discarded => ("discarded", None),
		ProgressOutcome::Interrupted(reason) => ("interrupted", Some(reason.as_str())),
	}
}

/// Read the Run that owns attempts. Writers call it under the Run row lock.
pub(crate) async fn target(executor: impl Executor, run: Uuid) -> Result<RunTarget> {
	rows::<RunTarget>(
		executor,
		Query::select()
			.columns(["id", "task_id", "workspace_id", "home_node", "control"].map(Alias::new))
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(run))),
	)
	.await?
	.pop()
	.ok_or_else(|| Error::NotFound("run".into()))
}

async fn next_seq(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<i64> {
	let next = rows::<NextSeq>(
		&mut *tx,
		Query::select()
			.expr_as(
				Func::coalesce(vec![
					Func::max(Expr::col("last_seq").into()),
					Expr::value(0_i64).into(),
				])
				.add(Expr::value(1_i64)),
				Alias::new("next"),
			)
			.from(Alias::new(InferenceAttempt::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run))),
	)
	.await?;
	Ok(next.first().map_or(1, |row| row.next))
}

async fn insert_row(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
	seq: i64,
	attempt: Uuid,
	kind: &str,
	item: Value,
) -> Result<()> {
	execute(
		tx,
		Query::insert()
			.into_table(Alias::new(InferenceProgressRecord::table_name()))
			.columns(["run_id", "seq", "attempt_id", "kind", "item", "created_at"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(run))
					.expr(Expr::value(seq))
					.expr(Expr::value(attempt))
					.expr(Expr::value(kind))
					.expr(Expr::value(item).cast_as("jsonb"))
					.expr(Expr::current_timestamp())
					.to_owned(),
			),
	)
	.await?;
	Ok(())
}

async fn marker(
	tx: &mut dyn TransactionExecutor,
	node: &str,
	run: &RunTarget,
	attempt: Uuid,
	kind: &str,
	reason: Option<&str>,
) -> Result<()> {
	let mut data = json!({
		"run_id": run.id,
		"task_id": run.task_id,
		"workspace_id": run.workspace_id,
		"attempt_id": attempt,
	});
	if let Some(reason) = reason {
		data["reason"] = json!(reason);
	}
	// The same workspace rule as the Run's own events keeps run-scoped visibility.
	event_records::append(
		tx,
		node,
		(run.home_node == node).then_some(run.workspace_id),
		kind,
		data,
	)
	.await
}

/// Write the single outcome of a pending attempt. Returns false when the
/// attempt already has one; outcomes never change.
async fn close(
	tx: &mut dyn TransactionExecutor,
	node: &str,
	run: &RunTarget,
	attempt: Uuid,
	outcome: ProgressOutcome,
) -> Result<bool> {
	let seq = next_seq(tx, run.id).await?;
	let (name, reason) = outcome_columns(outcome);
	let closed = execute(
		tx,
		Query::update()
			.table(Alias::new(InferenceAttempt::table_name()))
			.value_expr(Alias::new("outcome"), Expr::value(name))
			.value_expr(Alias::new("reason"), Expr::value(reason.map(str::to_owned)))
			.value_expr(Alias::new("finished_at"), Expr::current_timestamp())
			.value_expr(Alias::new("last_seq"), Expr::value(seq))
			.and_where(Expr::col("id").eq(Expr::value(attempt)))
			.and_where(Expr::col("run_id").eq(Expr::value(run.id)))
			.and_where(Expr::col("outcome").is_null()),
	)
	.await?;
	if closed != 1 {
		return Ok(false);
	}
	insert_row(tx, run.id, seq, attempt, OUTCOME, json!(outcome)).await?;
	marker(tx, node, run, attempt, outcome.event_kind(), reason).await?;
	Ok(true)
}

/// Close every pending attempt of a Run that its worker can no longer finish.
/// `None` derives the reason from Run control. Returns the outcomes written.
pub(crate) async fn close_pending(
	tx: &mut dyn TransactionExecutor,
	node: &str,
	run: Uuid,
	outcome: Option<ProgressOutcome>,
) -> Result<Vec<ProgressOutcome>> {
	let pending = rows::<PendingAttempt>(
		&mut *tx,
		Query::select()
			.column(Alias::new("id"))
			.from(Alias::new(InferenceAttempt::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("outcome").is_null())
			.lock(LockType::Update),
	)
	.await?;
	if pending.is_empty() {
		return Ok(Vec::new());
	}
	let target = target(&mut *tx, run).await?;
	let outcome = outcome.unwrap_or_else(|| target.orphan_outcome());
	let mut closed = Vec::new();
	for attempt in pending {
		if close(tx, node, &target, attempt.id, outcome).await? {
			closed.push(outcome);
		}
	}
	Ok(closed)
}

/// Record a pending attempt with its `started` row and marker, after closing any
/// attempt left pending by a lost lease.
pub(crate) async fn start(
	tx: &mut dyn TransactionExecutor,
	node: &str,
	run: Uuid,
	token: Uuid,
	attempt: InferenceAttemptId,
) -> Result<Vec<ProgressOutcome>> {
	let orphans = close_pending(
		tx,
		node,
		run,
		Some(ProgressOutcome::Interrupted(InterruptionReason::LeaseLost)),
	)
	.await?;
	let target = target(&mut *tx, run).await?;
	let seq = next_seq(tx, run).await?;
	execute(
		tx,
		Query::insert()
			.into_table(Alias::new(InferenceAttempt::table_name()))
			.columns(
				[
					"id",
					"run_id",
					"usage_attempt",
					"first_seq",
					"last_seq",
					"started_at",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(attempt.0))
					.expr(Expr::value(run))
					.expr(Expr::value(token))
					.expr(Expr::value(seq))
					.expr(Expr::value(seq))
					.expr(Expr::current_timestamp())
					.to_owned(),
			),
	)
	.await?;
	insert_row(
		tx,
		run,
		seq,
		attempt.0,
		STARTED,
		json!({"outcome": "pending"}),
	)
	.await?;
	marker(tx, node, &target, attempt.0, STARTED_EVENT, None).await?;
	Ok(orphans)
}

/// Split oversized text so each stored item stays within the row bound. Other
/// items larger than the bound are display-only and are dropped.
fn bounded(batch: &[InferenceProgress]) -> Vec<(&'static str, Value)> {
	let limit = MAX_PROGRESS_ITEM_BYTES - STORAGE_MARGIN;
	let mut items = Vec::with_capacity(batch.len());
	for progress in batch {
		match progress {
			InferenceProgress::Text { text } => {
				// PostgreSQL JSON cannot store NUL; it is never meaningful display text.
				let overhead = r#"{"type":"text","text":""}"#.len();
				let mut chunk = String::new();
				let mut size = overhead;
				for c in text.chars().filter(|c| *c != '\0') {
					let width = match c {
						'"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
						c if (c as u32) < 0x20 => 6,
						c => c.len_utf8(),
					};
					if size + width > limit {
						items.push((
							TEXT,
							json!(InferenceProgress::Text {
								text: std::mem::take(&mut chunk)
							}),
						));
						size = overhead;
					}
					chunk.push(c);
					size += width;
				}
				if !chunk.is_empty() {
					items.push((TEXT, json!(InferenceProgress::Text { text: chunk })));
				}
			}
			InferenceProgress::ToolCall { .. } => {
				let item = json!(progress);
				if item.to_string().len() <= limit {
					items.push((TOOL_CALL, item));
				} else {
					tracing::debug!("oversized tool-call progress dropped");
				}
			}
		}
	}
	items
}

/// Append progress to a pending attempt at the Run's next `progress_seq` values.
pub(crate) async fn append(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
	attempt: InferenceAttemptId,
	batch: &[InferenceProgress],
) -> Result<()> {
	let pending = rows::<PendingAttempt>(
		&mut *tx,
		Query::select()
			.column(Alias::new("id"))
			.from(Alias::new(InferenceAttempt::table_name()))
			.and_where(Expr::col("id").eq(Expr::value(attempt.0)))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("outcome").is_null())
			.lock(LockType::Update),
	)
	.await?;
	if pending.is_empty() {
		return Err(Error::Conflict("inference attempt is not pending".into()));
	}
	let items = bounded(batch);
	if items.is_empty() {
		return Ok(());
	}
	let mut seq = next_seq(tx, run).await?;
	for (kind, item) in items {
		insert_row(tx, run, seq, attempt.0, kind, item).await?;
		seq += 1;
	}
	execute(
		tx,
		Query::update()
			.table(Alias::new(InferenceAttempt::table_name()))
			.value_expr(Alias::new("last_seq"), Expr::value(seq - 1))
			.and_where(Expr::col("id").eq(Expr::value(attempt.0))),
	)
	.await?;
	Ok(())
}

/// Close one attempt without an Accepted Response. A closed attempt is unchanged.
pub(crate) async fn finish(
	tx: &mut dyn TransactionExecutor,
	node: &str,
	run: Uuid,
	attempt: InferenceAttemptId,
	outcome: ProgressOutcome,
) -> Result<bool> {
	if outcome == ProgressOutcome::Accepted {
		return Err(Error::Invalid(
			"only the saved model response accepts an inference attempt".into(),
		));
	}
	let target = target(&mut *tx, run).await?;
	close(tx, node, &target, attempt.0, outcome).await
}

/// Delete text and tool-call rows of attempts finished longer than the retention
/// window ago. `started` and `outcome` rows live as long as the Run.
pub(crate) async fn prune(tx: &mut dyn TransactionExecutor) -> Result<u64> {
	let retention = Expr::value(format!("{} seconds", RETENTION.as_secs())).cast_as("interval");
	let due = rows::<PendingAttempt>(
		&mut *tx,
		Query::select()
			.column(Alias::new("id"))
			.from(Alias::new(InferenceAttempt::table_name()))
			.and_where(Expr::col("outcome").is_not_null())
			.and_where(Expr::col("pruned_at").is_null())
			.and_where(
				SimpleExpr::from(Expr::col("finished_at"))
					.lt(Expr::current_timestamp().sub(retention)),
			)
			.order_by(Alias::new("finished_at"), Order::Asc)
			.limit(PRUNE_BATCH)
			.lock(LockType::Update)
			.lock_behavior(LockBehavior::SkipLocked),
	)
	.await?;
	if due.is_empty() {
		return Ok(0);
	}
	let ids: Vec<Uuid> = due.into_iter().map(|attempt| attempt.id).collect();
	execute(
		tx,
		Query::delete()
			.from_table(Alias::new(InferenceProgressRecord::table_name()))
			.and_where(Expr::col("attempt_id").is_in(ids.iter().copied().map(Expr::value)))
			.and_where(Expr::col("kind").is_in([TEXT, TOOL_CALL])),
	)
	.await?;
	execute(
		tx,
		Query::update()
			.table(Alias::new(InferenceAttempt::table_name()))
			.value_expr(Alias::new("pruned_at"), Expr::current_timestamp())
			.and_where(Expr::col("id").is_in(ids.iter().copied().map(Expr::value))),
	)
	.await?;
	Ok(ids.len() as u64)
}

/// Rows after `after` in `progress_seq` order, each with its attempt's outcome.
pub(crate) async fn page(
	executor: impl Executor,
	run: Uuid,
	after: i64,
	limit: u64,
) -> Result<Vec<ProgressRow>> {
	rows(
		executor,
		Query::select()
			.column((Alias::new("p"), Alias::new("seq")))
			.column((Alias::new("p"), Alias::new("attempt_id")))
			.column((Alias::new("p"), Alias::new("kind")))
			.column((Alias::new("p"), Alias::new("item")))
			.column((Alias::new("p"), Alias::new("created_at")))
			.column((Alias::new("a"), Alias::new("outcome")))
			.from_as(
				Alias::new(InferenceProgressRecord::table_name()),
				Alias::new("p"),
			)
			.join(
				JoinType::InnerJoin,
				reinhardt::query::TableRef::table_alias(
					Alias::new(InferenceAttempt::table_name()),
					Alias::new("a"),
				),
				Expr::col((Alias::new("a"), Alias::new("id")))
					.equals((Alias::new("p"), Alias::new("attempt_id"))),
			)
			.and_where(Expr::col((Alias::new("p"), Alias::new("run_id"))).eq(Expr::value(run)))
			.and_where(Expr::col((Alias::new("p"), Alias::new("seq"))).gt(Expr::value(after)))
			.order_by((Alias::new("p"), Alias::new("seq")), Order::Asc)
			.limit(limit),
	)
	.await
}

struct Bounds {
	first_seq: Option<i64>,
	pending: bool,
}
crate::native_record!(Bounds { first_seq, pending });

/// The latest attempt's first sequence and whether an attempt is still pending.
pub(crate) async fn bounds(executor: impl Executor, run: Uuid) -> Result<(Option<i64>, bool)> {
	let found = rows::<Bounds>(
		executor,
		Query::select()
			.expr_as(
				Func::max(Expr::col("first_seq").into()),
				Alias::new("first_seq"),
			)
			.expr_as(
				Func::coalesce(vec![
					SimpleExpr::FunctionCall(
						"bool_or".into_iden(),
						vec![Expr::col("outcome").is_null()],
					),
					Expr::value(false).into(),
				]),
				Alias::new("pending"),
			)
			.from(Alias::new(InferenceAttempt::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run))),
	)
	.await?;
	Ok(found
		.into_iter()
		.next()
		.map_or((None, false), |row| (row.first_seq, row.pending)))
}

/// Count interruptions the server writes itself; the harness counts its own.
pub(crate) fn record_interruptions(closed: &[ProgressOutcome]) {
	for outcome in closed {
		if *outcome != ProgressOutcome::Accepted {
			metrics::counter!("aidash_inference_interruptions_total", "reason" => outcome.reason())
				.increment(1);
		}
	}
}

async fn fence(tx: &mut dyn TransactionExecutor, run: Uuid, token: Uuid) -> Result<()> {
	if crate::apps::execution::models::Run::hold_worker(tx, run, token).await? {
		Ok(())
	} else {
		Err(Error::Conflict("worker lease lost".into()))
	}
}

impl crate::store::Store {
	pub async fn start_inference(
		&self,
		run: Uuid,
		token: Uuid,
		attempt: InferenceAttemptId,
	) -> Result<()> {
		let mut tx = native::begin(&self.pool).await?;
		fence(tx.as_mut(), run, token).await?;
		let orphans = start(tx.as_mut(), &self.node_id, run, token, attempt).await?;
		tx.commit().await?;
		record_interruptions(&orphans);
		Ok(())
	}

	pub async fn append_inference_progress(
		&self,
		run: Uuid,
		token: Uuid,
		attempt: InferenceAttemptId,
		batch: &[InferenceProgress],
	) -> Result<()> {
		let mut tx = native::begin(&self.pool).await?;
		fence(tx.as_mut(), run, token).await?;
		append(tx.as_mut(), run, attempt, batch).await?;
		tx.commit().await
	}

	pub async fn finish_inference(
		&self,
		run: Uuid,
		token: Uuid,
		attempt: InferenceAttemptId,
		outcome: ProgressOutcome,
	) -> Result<()> {
		let mut tx = native::begin(&self.pool).await?;
		fence(tx.as_mut(), run, token).await?;
		finish(tx.as_mut(), &self.node_id, run, attempt, outcome).await?;
		tx.commit().await
	}

	/// After a stale response rolls back, close its attempt as discarded under
	/// the same lease. A lost lease leaves it to orphan closure.
	pub(crate) async fn discard_stale_inference(&self, run: Uuid, token: Uuid) {
		let result = async {
			let mut tx = native::begin(&self.pool).await?;
			fence(tx.as_mut(), run, token).await?;
			let closed = close_pending(
				tx.as_mut(),
				&self.node_id,
				run,
				Some(ProgressOutcome::Discarded),
			)
			.await?;
			tx.commit().await?;
			record_interruptions(&closed);
			Ok::<_, Error>(())
		}
		.await;
		if let Err(error) = result {
			tracing::warn!(run_id = %run, %error, "stale inference attempt left for orphan closure");
		}
	}

	/// One bounded retention pass. Returns the number of attempts pruned.
	pub async fn prune_inference_progress(&self) -> Result<u64> {
		let mut tx = native::begin(&self.pool).await?;
		let pruned = prune(tx.as_mut()).await?;
		tx.commit().await?;
		Ok(pruned)
	}

	pub(crate) async fn inference_page(
		&self,
		run: Uuid,
		after: i64,
		limit: u64,
	) -> Result<Vec<ProgressRow>> {
		page(&self.pool, run, after, limit).await
	}

	pub(crate) async fn inference_bounds(&self, run: Uuid) -> Result<(Option<i64>, bool)> {
		bounds(&self.pool, run).await
	}

	pub(crate) async fn inference_run(&self, run: Uuid) -> Result<RunTarget> {
		target(&self.pool, run).await
	}
}
