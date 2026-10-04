//! Document persistence within the caller's authorization transaction.
use crate::{Error, Result};
use reinhardt::query::{
	Alias, Expr, ExprTrait, OnConflict, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sqlx::{Postgres, Transaction};

pub(crate) async fn get<T: DeserializeOwned>(
	tx: &mut Transaction<'_, Postgres>,
	table: &str,
	key: &str,
) -> Result<Option<T>> {
	let value: Option<Value> = {
		let query_bind_1 = key;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("document"))
				.from(Alias::new(table))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("key"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **tx)
		.await?
	};
	value
		.map(serde_json::from_value)
		.transpose()
		.map_err(Into::into)
}

pub(crate) async fn documents_page<T: DeserializeOwned>(
	tx: &mut Transaction<'_, Postgres>,
	table: &str,
	after: &str,
	limit: u64,
) -> Result<Vec<(String, T)>> {
	let rows: Vec<(String, Value)> = sqlx::query_as(
		&Query::select()
			.columns([Alias::new("key"), Alias::new("document")])
			.from(Alias::new(table))
			.and_where(Expr::col(Alias::new("key")).gt(Expr::value(after)))
			.order_by(Alias::new("key"), Order::Asc)
			.limit(limit)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **tx)
	.await?;
	rows.into_iter()
		.map(|(key, value)| Ok((key, serde_json::from_value(value)?)))
		.collect()
}

pub(crate) async fn put(
	tx: &mut Transaction<'_, Postgres>,
	table: &str,
	key: &str,
	value: &impl Serialize,
) -> Result<()> {
	if table == "marketplace_installations" {
		let updated = {
			let query_bind_1 = key;
			let query_bind_2 = serde_json::to_value(value)?;
			sqlx::query(
				&Query::update()
					.table(Alias::new(table))
					.value_expr(
						Alias::new("document"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(key=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **tx)
			.await?
		};
		if updated.rows_affected() != 1 {
			return Err(Error::Forbidden);
		}
		return Ok(());
	}
	{
		let query_bind_1 = key;
		let query_bind_2 = serde_json::to_value(value)?;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new(table))
				.columns([Alias::new("key"), Alias::new("document")])
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
						.to_owned(),
				)
				.on_conflict(
					OnConflict::column(Alias::new("key"))
						.update_columns([Alias::new("document")])
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await?
	};
	Ok(())
}

/// Preserve the legacy operator adoption snapshot on its existing transaction.
pub(crate) async fn effective_legacy(
	tx: &mut Transaction<'_, Postgres>,
	id: &str,
	version: &str,
) -> Result<crate::registry::Entry> {
	let sql = Query::select()
		.column(Alias::new("metadata"))
		.from(Alias::new("registry"))
		.and_where(Expr::col("id").eq(Expr::value(id)))
		.and_where(Expr::col("version").eq(Expr::value(version)))
		.to_string(PostgresQueryBuilder);
	let value: Value = sqlx::query_scalar(&sql).fetch_one(&mut **tx).await?;
	let mut entry: crate::registry::Entry = serde_json::from_value(value)?;
	let sql = Query::select()
		.column(Alias::new("config"))
		.from(Alias::new("installations"))
		.and_where(Expr::col("id").eq(Expr::value(id)))
		.and_where(Expr::col("version").eq(Expr::value(version)))
		.to_string(PostgresQueryBuilder);
	if let Some(config) = sqlx::query_scalar::<_, Value>(&sql)
		.fetch_optional(&mut **tx)
		.await?
	{
		crate::registry::overlay_config(&mut entry.config, &config)?;
	}
	Ok(entry)
}

use reinhardt::query::SimpleExpr;
