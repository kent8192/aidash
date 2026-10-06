//! Idempotently erase old JSON-body projections before the application becomes available.
//! This performs deletion only; it never imports or converts a legacy memory.
use crate::{Result, database::native};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};

pub(crate) async fn legacy(pool: &native::Pool) -> Result<()> {
	let mut tx = native::begin(pool).await?;
	let ids = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("semantic_entries"))
		.and_where(Expr::col("key").like("agent-memory:%"))
		.to_owned();
	// Keep revision tombstones and read dependencies so old consumers remain invalidated.
	native::query(
		&Query::update()
			.table(Alias::new("semantic_points"))
			.value_expr(Alias::new("retired"), Expr::value(true))
			.and_where(Expr::col("entry_id").in_subquery(ids.clone()))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	native::query(
		&Query::update()
			.table(Alias::new("semantic_entries"))
			.value_expr(
				Alias::new("source"),
				SimpleExpr::FunctionCall(
					"jsonb_build_object".into_iden(),
					vec![
						Expr::value("kind").into(),
						Expr::value("unit").into(),
						Expr::value("id").into(),
						Expr::col("id").into(),
					],
				),
			)
			.value_expr(Alias::new("deleted"), Expr::value(true))
			.value_expr(Alias::new("state"), Expr::value("DELETED"))
			.value_expr(Alias::new("revision"), Expr::col("revision").add(1_i64))
			.and_where(Expr::col("id").in_subquery(ids))
			.and_where(Expr::col("deleted").eq(false))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	tx.commit().await
}
