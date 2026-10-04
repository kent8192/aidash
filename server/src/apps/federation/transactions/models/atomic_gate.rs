//! Persistent atomic_gate records.

use crate::{Error, Result};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::{DatabaseConnection, TransactionExecutor};
use reinhardt::db::orm::Model as ModelTrait;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::macros::Model;
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockBehavior, LockType, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Model, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[model_config(app_label = "federation", table_name = "atomic_gate")]
pub struct AtomicGate {
	#[field(primary_key = true, default = true)]
	pub singleton: bool,

	#[field(db_column = "transaction_id", null = true)]
	pub transaction_id: Option<uuid::Uuid>,
	#[field(default = 0)]
	pub commit_epoch: i64,
}

impl AtomicGate {
	pub(crate) async fn lock_exclusive(tx: &mut dyn TransactionExecutor) -> Result<Option<Uuid>> {
		let row = Self::objects()
			.filter(Self::field_singleton().eq(true))
			.select_for_update()
			.all_with_executor(tx)
			.await
			.map_err(|error| {
				if error.code() == Some("55P03") {
					Error::TransactionPending
				} else {
					Error::from(FrameworkError::from(error))
				}
			})?
			.pop()
			.ok_or_else(|| Error::NotFound("atomic visibility gate".into()))?;
		Ok(row.transaction_id)
	}

	pub(crate) async fn reserve(tx: &mut dyn TransactionExecutor, id: Uuid) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new("atomic_gate"))
			.value_expr(Alias::new("transaction_id"), Expr::value(id))
			.and_where(Expr::col("singleton").eq(reinhardt::query::Expr::value(true)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	pub(crate) async fn release(tx: &mut dyn TransactionExecutor, committed: bool) -> Result<()> {
		let mut query = Query::update();
		query
			.table(Alias::new("atomic_gate"))
			.value_expr(Alias::new("transaction_id"), Expr::value(None::<Uuid>))
			.and_where(Expr::col("singleton").eq(reinhardt::query::Expr::value(true)));
		if committed {
			query.value_expr(
				Alias::new("commit_epoch"),
				Expr::col("commit_epoch").add(reinhardt::query::Expr::value(1_i64)),
			);
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	/// Retain a shared row lock until the protected operation releases its lease.
	pub(crate) async fn read_lease(
		db: &DatabaseConnection,
	) -> Result<(Box<dyn TransactionExecutor>, i64)> {
		let mut tx = db.begin().await?;
		// The ORM's select_for_update cannot express FOR SHARE NOWAIT in alpha.16.
		let (sql, values) = Query::select()
			.expr_as(
				Expr::col(Alias::new("transaction_id")).is_not_null(),
				Alias::new("pending"),
			)
			.column(Alias::new("commit_epoch"))
			.from(Alias::new("atomic_gate"))
			.and_where(Expr::col(Alias::new("singleton")).eq(reinhardt::query::Expr::value(true)))
			.lock(LockType::Share)
			.lock_behavior(LockBehavior::Nowait)
			.build(PostgresQueryBuilder);
		let row = tx
			.fetch_one(&sql, convert_values(values))
			.await
			.map_err(|error| {
				if error
					.database_error()
					.is_some_and(|error| error.code() == Some("55P03"))
				{
					Error::TransactionPending
				} else {
					Error::from(error)
				}
			})?;
		if row.get::<bool>("pending").map_err(FrameworkError::from)? {
			tx.rollback().await?;
			return Err(Error::TransactionPending);
		}
		let epoch = row
			.get::<i64>("commit_epoch")
			.map_err(FrameworkError::from)?;
		Ok((tx, epoch))
	}
}
