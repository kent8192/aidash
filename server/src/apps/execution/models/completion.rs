//! Fenced execution completion on a caller-owned native transaction.
use super::Run;
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, Expr, ExprTrait, Func, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use uuid::Uuid;

impl Run {
	pub(crate) async fn lock_with_lease(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
	) -> Result<(Self, bool)> {
		let run = Self::objects()
			.filter(Self::field_id().eq(id))
			.select_for_update()
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.ok_or_else(|| Error::Conflict("run unavailable".into()))?;
		let (sql, values) = Query::select()
			.expr_as(
				Func::coalesce(vec![
					Expr::col("lease_until").gt(SimpleExpr::FunctionCall(
						"clock_timestamp".into_iden(),
						vec![],
					)),
					Expr::value(false).into(),
				]),
				Alias::new("leased"),
			)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.build(PostgresQueryBuilder);
		let row = tx.fetch_one(&sql, convert_values(values)).await?;
		Ok((run, row.get("leased").map_err(FrameworkError::from)?))
	}

	/// Complete only after the caller has locked and validated the execution.
	pub(crate) async fn complete(tx: &mut dyn TransactionExecutor, id: Uuid) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("phase"), Expr::value("COMPLETED"))
			.value_expr(
				Alias::new("pending"),
				Expr::value(crate::domain::run_state::encode(
					&crate::domain::RunState::Completed(crate::domain::TerminalState {}),
					&crate::domain::RecoveryState::default(),
				)?),
			)
			.value_expr(Alias::new("error"), Expr::value(None::<String>))
			.value_expr(
				Alias::new("revision"),
				Expr::col("revision").add(reinhardt::query::Expr::value(1_i64)),
			)
			.value_expr(Alias::new("lease_owner"), Expr::value(None::<Uuid>))
			.value_expr(
				Alias::new("lease_until"),
				Expr::value(None::<DateTime<Utc>>),
			)
			.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}
}
