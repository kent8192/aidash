//! Run and human-input projections on the caller's authorization transaction.
use super::{HumanRequest, Run};
use crate::{Result, domain};
use chrono::{DateTime, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, QueryRow};
use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use uuid::Uuid;

impl Run {
	pub(crate) async fn read_in(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		workspace: Option<Uuid>,
	) -> Result<Option<domain::Run>> {
		let mut query = Self::objects().filter(Self::field_id().eq(id));
		if let Some(workspace) = workspace {
			query = query.filter(Self::field_workspace_id().eq(workspace));
		}
		query
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.map(TryInto::try_into)
			.transpose()
	}

	pub(crate) async fn agent_page(
		tx: &mut dyn TransactionExecutor,
		agent: &str,
		version: &str,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<domain::RunMetadata>> {
		let mut query = Query::select();
		query
			.column(ColumnRef::Asterisk)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("agent_id").eq(Expr::value(agent)))
			.and_where(Expr::col("agent_version").eq(Expr::value(version)))
			.order_by(Alias::new("updated_at"), Order::Desc)
			.order_by(Alias::new("id"), Order::Desc)
			.limit(501);
		if let Some((time, id)) = cursor {
			query.cond_where(
				Condition::any()
					.add(Expr::col("updated_at").lt(Expr::value(time)))
					.add(
						Condition::all()
							.add(Expr::col("updated_at").eq(Expr::value(time)))
							.add(Expr::col("id").lt(Expr::value(id))),
					),
			);
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		Ok(tx
			.fetch_all(&sql, convert_values(values))
			.await?
			.into_iter()
			.map(|row| serde_json::from_value(QueryRow::from_backend_row(row).data))
			.collect::<std::result::Result<_, _>>()?)
	}
}

impl HumanRequest {
	pub(crate) async fn for_run(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		workspace: Uuid,
	) -> Result<Vec<domain::HumanRequest>> {
		let (sql, values) = Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.order_by(Alias::new("id"), Order::Asc)
			.build(PostgresQueryBuilder);
		Ok(tx
			.fetch_all(&sql, convert_values(values))
			.await?
			.into_iter()
			.map(|row| serde_json::from_value(QueryRow::from_backend_row(row).data))
			.collect::<std::result::Result<_, _>>()?)
	}
}
