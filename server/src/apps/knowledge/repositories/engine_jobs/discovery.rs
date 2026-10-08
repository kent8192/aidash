//! A finite discovery cycle prevents a revoked/full oldest bank from starving later Runs.
use crate::{Result, database::native, store::Store};
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait, LockBehavior, LockType, OnConflict, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

pub(super) async fn page(store: &Store) -> Result<Vec<Uuid>> {
	let epoch = DateTime::<Utc>::UNIX_EPOCH;
	let mut tx = native::begin(&store.pool).await?;
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_learning_discovery"))
			.columns(["home", "cursor_updated", "cursor_id", "cycle_before"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(&store.node_id))
					.expr(Expr::value(epoch))
					.expr(Expr::value(Uuid::nil()))
					.expr(Expr::value(Utc::now()))
					.to_owned(),
			)
			.on_conflict(
				OnConflict::column(Alias::new("home"))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	let Some(cursor) = native::query(
		&Query::select()
			.columns(["cursor_updated", "cursor_id", "cycle_before"].map(Alias::new))
			.from(Alias::new("memory_learning_discovery"))
			.and_where(Expr::col("home").eq(store.node_id.as_str()))
			.lock(LockType::Update)
			.lock_behavior(LockBehavior::SkipLocked)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut *tx)
	.await?
	else {
		return Ok(vec![]);
	};
	let attempted = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("memory_engine_jobs"))
		.and_where(Expr::col("kind").eq("learn"))
		.to_owned();
	let updated: DateTime<Utc> = cursor.try_get("cursor_updated")?;
	let id: Uuid = cursor.try_get("cursor_id")?;
	let before: DateTime<Utc> = cursor.try_get("cycle_before")?;
	let rows = native::query(
		&Query::select()
			.columns(["id", "updated_at"].map(Alias::new))
			.from(Alias::new("runs"))
			.and_where(Expr::col("home_node").eq(store.node_id.as_str()))
			.and_where(Expr::col("phase").eq("COMPLETED"))
			.and_where(Expr::col("id").not_in_subquery(attempted))
			.and_where(
				Expr::col("id").in_subquery(
					Query::select()
						.column(Alias::new("run_id"))
						.from(Alias::new("memory_run_bindings"))
						.to_owned(),
				),
			)
			.and_where(Expr::col("updated_at").lte(Expr::value(before)))
			.cond_where(
				Condition::any()
					.add(Expr::col("updated_at").gt(Expr::value(updated)))
					.add(
						Condition::all()
							.add(Expr::col("updated_at").eq(Expr::value(updated)))
							.add(Expr::col("id").gt(Expr::value(id))),
					),
			)
			.order_by(Alias::new("updated_at"), Order::Asc)
			.order_by(Alias::new("id"), Order::Asc)
			.limit(32)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut *tx)
	.await?;
	let mut update = Query::update();
	update
		.table(Alias::new("memory_learning_discovery"))
		.and_where(Expr::col("home").eq(store.node_id.as_str()));
	if let Some(last) = rows.last() {
		update
			.value(
				Alias::new("cursor_updated"),
				last.try_get::<DateTime<Utc>>("updated_at")?,
			)
			.value(Alias::new("cursor_id"), last.try_get::<Uuid>("id")?);
	} else {
		update
			.value(Alias::new("cursor_updated"), epoch)
			.value(Alias::new("cursor_id"), Uuid::nil())
			.value(Alias::new("cycle_before"), Utc::now());
	}
	native::query(&update.to_string(PostgresQueryBuilder))
		.execute(&mut *tx)
		.await?;
	// Advance before processing. A process cut retries these identities next cycle;
	// it cannot make a failed earliest page an indefinite head-of-line block.
	tx.commit().await?;
	rows.into_iter().map(|row| row.try_get("id")).collect()
}
