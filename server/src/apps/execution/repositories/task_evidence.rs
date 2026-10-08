//! Execution evidence retains the supplied version, never the latest task prose.
use crate::{Error, Result, database::native};
use aidash_domain::{Run, Task};
use reinhardt::query::{
	Alias, Expr, ExprTrait, OnConflict, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use sha2::{Digest, Sha256};

pub(crate) async fn record(tx: &mut native::Transaction, run: &Run, task: &Task) -> Result<()> {
	if run.task_id != task.id || run.workspace_id != task.workspace_id || task.revision < 0 {
		return Err(Error::Forbidden);
	}
	let mut hash = Sha256::new();
	hash.update(b"run-task-snapshot-v1");
	hash.update(run.id.as_bytes());
	hash.update(run.step.to_le_bytes());
	hash.update(run.observed_input_seq.to_le_bytes());
	hash.update(task.revision.to_le_bytes());
	let mut bytes = [0; 16];
	bytes.copy_from_slice(&hash.finalize()[..16]);
	bytes[6] = (bytes[6] & 0x0f) | 0x80;
	bytes[8] = (bytes[8] & 0x3f) | 0x80;
	let id = uuid::Uuid::from_bytes(bytes);
	let body = serde_json::to_value(task)?;
	native::query(
		&Query::insert()
			.into_table(Alias::new("run_task_snapshots"))
			.columns(
				[
					"id",
					"run_id",
					"task_revision",
					"step",
					"input_seq",
					"body",
					"captured_at",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(id))
					.expr(Expr::value(run.id))
					.expr(Expr::value(task.revision))
					.expr(Expr::value(run.step))
					.expr(Expr::value(run.observed_input_seq))
					.expr(Expr::value(body.clone()))
					.expr(Expr::value(chrono::Utc::now()))
					.to_owned(),
			)
			.on_conflict(OnConflict::column(Alias::new("id")).do_nothing().to_owned())
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **tx)
	.await?;
	let saved: serde_json::Value = native::query_scalar(
		&Query::select()
			.column(Alias::new("body"))
			.from(Alias::new("run_task_snapshots"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **tx)
	.await?;
	if saved != body {
		return Err(Error::Conflict("observed task snapshot changed".into()));
	}
	Ok(())
}

pub(crate) async fn complete(
	tx: &mut native::Transaction,
	run: uuid::Uuid,
	limit: usize,
) -> Result<Vec<super::super::models::task_evidence::RunTaskSnapshot>> {
	let records = native::query(
		&Query::select()
			.column(reinhardt::query::ColumnRef::Asterisk)
			.from(Alias::new("run_task_snapshots"))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.order_by(Alias::new("captured_at"), Order::Asc)
			.order_by(Alias::new("id"), Order::Asc)
			.limit(limit as u64 + 1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **tx)
	.await?;
	records
		.into_iter()
		.map(|row| {
			Ok(super::super::models::task_evidence::RunTaskSnapshot {
				id: row.try_get("id")?,
				run_id: row.try_get("run_id")?,
				task_revision: row.try_get("task_revision")?,
				step: row.try_get("step")?,
				input_seq: row.try_get("input_seq")?,
				body: row.try_get("body")?,
				captured_at: row.try_get("captured_at")?,
			})
		})
		.collect()
}
