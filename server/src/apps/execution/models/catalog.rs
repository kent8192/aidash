//! Native execution and event reads for the application's public projections.
use super::{Event, Run, RunInput};
use crate::{Error, Result};
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, OrmExecutor};
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

impl Run {
	pub(crate) async fn read<E: OrmExecutor>(db: &mut E, id: Uuid) -> Result<Self> {
		Self::objects()
			.filter(Self::field_id().eq(id))
			.first_with_db(db)
			.await?
			.ok_or_else(|| Error::NotFound("run".into()))
	}
}

impl Event {
	pub(crate) async fn in_workspace<E: OrmExecutor>(
		db: &mut E,
		workspace: Uuid,
		id: Uuid,
	) -> Result<Self> {
		Self::objects()
			.filter(Self::field_id().eq(id))
			.filter(Self::field_workspace_id().eq(Some(workspace)))
			.first_with_db(db)
			.await?
			.ok_or_else(|| Error::Invalid("workspace record not available".into()))
	}

	pub(crate) async fn after<E: OrmExecutor>(
		db: &mut E,
		after: i64,
		workspace: Option<Uuid>,
		limit: i64,
	) -> Result<Vec<Self>> {
		let mut query = Self::objects().filter(Self::field_sequence().gt(after.max(0)));
		if let Some(workspace) = workspace {
			query = query.filter(Self::field_workspace_id().eq(Some(workspace)));
		}
		Ok(query
			.order_by(&["sequence"])
			.limit(limit.clamp(1, 1000) as usize)
			.all_with_db(db)
			.await?)
	}

	pub(crate) async fn recent<E: OrmExecutor>(db: &mut E, workspace: Uuid) -> Result<Vec<Self>> {
		let mut events = Self::objects()
			.filter(Self::field_workspace_id().eq(Some(workspace)))
			.order_by(&["-sequence"])
			.limit(100)
			.all_with_db(db)
			.await?;
		events.reverse();
		Ok(events)
	}
}

impl RunInput {
	pub(crate) async fn matching_sequence<E: OrmExecutor>(
		db: &mut E,
		run: Uuid,
		key: &str,
		content: &str,
	) -> Result<i64> {
		Self::objects()
			.filter(Self::field_run_id().eq(run))
			.filter(Self::field_idempotency_key().eq(key))
			.filter(Self::field_content().eq(content))
			.first_with_db(db)
			.await?
			.map(|row| row.seq)
			.ok_or_else(|| Error::Conflict("run message has no matching durable admission".into()))
	}

	pub(crate) async fn high_watermark<E: OrmExecutor>(db: &mut E, run: Uuid) -> Result<i64> {
		Ok(Self::objects()
			.filter(Self::field_run_id().eq(run))
			.order_by(&["-seq"])
			.first_with_db(db)
			.await?
			.map_or(0, |row| row.seq))
	}

	pub(crate) async fn defer_delivery<E: OrmExecutor>(db: &mut E, run: Uuid) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(
				Alias::new("delivery_retry_at"),
				Expr::current_timestamp()
					.add(Expr::value("5 seconds").cast_as(Alias::new("interval"))),
			)
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("message_id").is_null())
			.build(PostgresQueryBuilder);
		db.execute(&sql, convert_values(values)).await?;
		Ok(())
	}
}
