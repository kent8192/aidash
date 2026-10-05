//! Original leased Query primitives for semantic index and point persistence.
use crate::apps::knowledge::services::core::{Entry, Index};
use crate::{Error, Result};
use reinhardt::query::{
	Alias, ColumnRef, Expr, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};

use uuid::Uuid;

pub(crate) async fn index(
	tx: &mut crate::database::native::Transaction,
	workspace: Uuid,
	exclusive: bool,
) -> Result<Index> {
	{
		let query_bind_1 = workspace;
		crate::database::native::query_as(&if exclusive {
			Query::select()
				.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
				.from(Alias::new("semantic_indexes"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(workspace_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder)
		} else {
			Query::select()
				.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
				.from(Alias::new("semantic_indexes"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(workspace_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder)
		})
		.fetch_optional(&mut **tx)
		.await?
	}
	.ok_or_else(|| Error::NotFound("semantic index".into()))
}

pub(crate) async fn history(
	tx: &mut crate::database::native::Transaction,
	workspace: Uuid,
	entry: Option<Uuid>,
	revision: i64,
	state: &str,
	detail: &str,
) -> Result<()> {
	{
		let query_bind_1 = workspace;
		let query_bind_2 = entry;
		let query_bind_3 = revision;
		let query_bind_4 = state;
		let query_bind_5 = detail;
		crate::database::native::query(
			&Query::insert()
				.into_table(Alias::new("semantic_history"))
				.columns([
					Alias::new("workspace_id"),
					Alias::new("entry_id"),
					Alias::new("revision"),
					Alias::new("state"),
					Alias::new("detail"),
				])
				.from_subquery(
					Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_5.to_owned()).into()],
						))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await?
	};
	Ok(())
}

pub(crate) async fn schedule_point(
	tx: &mut crate::database::native::Transaction,
	entry: &Entry,
	collection: &str,
) -> Result<()> {
	{
		let query_bind_1 = entry.id;
		crate::database::native::query(
			&Query::update()
				.table(Alias::new("semantic_points"))
				.value_expr(Alias::new("retired"), Expr::cust("TRUE"))
				.value_expr(Alias::new("next_attempt"), Expr::cust("CLOCK_TIMESTAMP()"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(entry_id = ? AND NOT retired)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await?
	};
	if !entry.deleted {
		{
			let query_bind_1 = entry.point_id;
			let query_bind_2 = entry.id;
			let query_bind_3 = collection;
			crate::database::native::query(
				&Query::insert()
					.into_table(Alias::new("semantic_points"))
					.columns([
						Alias::new("id"),
						Alias::new("entry_id"),
						Alias::new("collection"),
					])
					.from_subquery(
						Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()],
							))
							.to_owned(),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **tx)
			.await?
		};
	}
	Ok(())
}
