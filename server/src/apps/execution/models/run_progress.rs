//! Worker-fenced progress writes on caller-owned native transactions.
use super::{Run, RunInput};
use crate::{Error, Result};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{Model, OrmExecutor, execution::convert_values};
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait, IntoIden, LockType, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr,
};
use uuid::Uuid;

fn worker_lease(id: Uuid, worker: Uuid) -> Condition {
	Condition::all()
		.add(Expr::col("id").eq(Expr::value(id)))
		.add(Expr::col("lease_owner").eq(Expr::value(worker)))
		.add(
			reinhardt::query::SimpleExpr::from(Expr::col("lease_until"))
				.gt(Expr::current_timestamp()),
		)
}

impl RunInput {
	pub(crate) async fn newer_than(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		sequence: i64,
	) -> Result<bool> {
		let newer = Query::select()
			.expr(Expr::value(1_i64))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("seq").gt(Expr::value(sequence)))
			.to_owned();
		let (sql, values) = Query::select()
			.expr_as(Expr::exists(newer), Alias::new("stale"))
			.build(PostgresQueryBuilder);
		Ok(tx
			.fetch_one(&sql, convert_values(values))
			.await?
			.get("stale")
			.map_err(FrameworkError::from)?)
	}
}

impl Run {
	/// Heartbeats identify the committed owner without copying the execution context.
	pub(crate) async fn leased_id<E: OrmExecutor>(
		db: &mut E,
		worker: Uuid,
	) -> Result<Option<Uuid>> {
		let (sql, values) = Query::select()
			.column(Alias::new("id"))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("lease_owner").eq(Expr::value(worker)))
			.build(PostgresQueryBuilder);
		db.fetch_optional(&sql, convert_values(values))
			.await?
			.map(|row| {
				row.get("id")
					.map_err(|error| Error::from(FrameworkError::from(error)))
			})
			.transpose()
	}

	/// Read committed control outside the worker's retained authority transaction.
	pub(crate) async fn committed_control<E: OrmExecutor>(
		db: &mut E,
		id: Uuid,
	) -> Result<aidash_domain::RunControl> {
		let (sql, values) = Query::select()
			.column(Alias::new("control"))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.build(PostgresQueryBuilder);
		let row = db.fetch_one(&sql, convert_values(values)).await?;
		let control: String = row.get("control").map_err(FrameworkError::from)?;
		Ok(serde_json::from_value(serde_json::Value::String(control))?)
	}

	pub(crate) async fn hold_worker(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		worker: Uuid,
	) -> Result<bool> {
		let (sql, values) = Query::select()
			.column(Alias::new("id"))
			.from(Alias::new(Self::table_name()))
			.and_where(worker_lease(id, worker))
			.lock(LockType::Update)
			.build(PostgresQueryBuilder);
		Ok(tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.is_some())
	}

	pub(crate) async fn ensure_response_current(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		worker: Uuid,
		sequence: i64,
	) -> Result<()> {
		if !Self::hold_worker(tx, id, worker).await? {
			return Err(Error::Conflict(
				"worker lease lost before response effect".into(),
			));
		}
		if RunInput::newer_than(tx, id, sequence).await? {
			return Err(Error::StaleInference);
		}
		Ok(())
	}

	pub(crate) async fn worker_context(tx: &mut dyn TransactionExecutor) -> Result<()> {
		let (sql, values) = Query::select()
			.expr(SimpleExpr::FunctionCall(
				"set_config".into_iden(),
				vec![
					Expr::value("aidash.input_ledger_worker").into(),
					Expr::value("true").into(),
					Expr::value(true).into(),
				],
			))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	pub(crate) async fn renew(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		worker: Uuid,
		seconds: i32,
	) -> Result<bool> {
		Self::worker_context(tx).await?;
		let duration = Expr::value(format!("{seconds} seconds")).cast_as("interval");
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(
				Alias::new("lease_until"),
				Expr::current_timestamp().add(duration),
			)
			.and_where(worker_lease(id, worker))
			.build(PostgresQueryBuilder);
		Ok(tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected
			== 1)
	}

	pub(crate) async fn release_worker(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		worker: Uuid,
	) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("lease_owner"), Expr::null())
			.value_expr(Alias::new("lease_until"), Expr::null())
			.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("lease_owner").eq(Expr::value(worker)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}
}
