use super::sessions;
use crate::{Error, Result, authorization::access::Access};
use chrono::{DateTime, Utc};
use reinhardt::query::{Alias, Expr, LockType, PostgresQueryBuilder, Query};
use serde_json::Value;
use uuid::Uuid;

pub(crate) async fn get(access: &mut Access, id: Uuid, kind: &str) -> Result<Record> {
	{
		let query_bind_1 = id;
		let query_bind_2 = &access.identity.tenant;
		let query_bind_3 = kind;
		sqlx::query_as(
			&sessions::select("core_records")
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					),
				)
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("kind"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					),
				)
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **access.tx)
		.await?
	}
	.ok_or_else(|| Error::NotFound("resource unavailable".into()))
}
pub(crate) async fn insert(
	access: &mut Access,
	id: Uuid,
	area: Option<Uuid>,
	kind: &str,
	state: &str,
	data: Value,
	expires_at: Option<DateTime<Utc>>,
) -> Result<Record> {
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("core_records"))
			.columns(
				[
					"id",
					"tenant",
					"owner",
					"area_id",
					"kind",
					"state",
					"data",
					"expires_at",
				]
				.map(Alias::new),
			)
			.from_subquery(((1..=8).map(|i| Expr::cust(format!("${i}")))).fold(
				reinhardt::query::Query::select(),
				|mut select, expr| {
					select.expr(expr);
					select
				},
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(&access.identity.tenant)
	.bind(&access.identity.subject)
	.bind(area)
	.bind(kind)
	.bind(state)
	.bind(data)
	.bind(expires_at)
	.execute(&mut **access.tx)
	.await?;
	get(access, id, kind).await
}
pub(crate) async fn update(access: &mut Access, record: &mut Record) -> Result<()> {
	update_committed(&mut access.tx, record).await
}
pub(crate) async fn update_committed(
	tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
	record: &mut Record,
) -> Result<()> {
	let changed = {
		let query_bind_1 = record.id;
		let query_bind_2 = &record.state;
		let query_bind_3 = &record.data;
		let query_bind_4 = record.expires_at;
		let query_bind_5 = record.revision;
		sqlx::query(
			&Query::update()
				.table(Alias::new("core_records"))
				.value_expr(Alias::new("state"), Expr::value(query_bind_2.to_owned()))
				.value_expr(Alias::new("data"), Expr::value(query_bind_3.to_owned()))
				.value_expr(
					Alias::new("expires_at"),
					Expr::value(query_bind_4.to_owned()),
				)
				.value_expr(
					Alias::new("revision"),
					Expr::col(Alias::new("revision")).add(reinhardt::query::Expr::value(1)),
				)
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())))
				.and_where(
					Expr::col(Alias::new("revision")).eq(Expr::value(query_bind_5.to_owned())),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await?
	}
	.rows_affected();
	if changed != 1 {
		return Err(Error::Conflict("RECORD_CHANGED".into()));
	}
	record.revision += 1;
	Ok(())
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

pub use crate::apps::execution::capabilities::serializers::records::Record;

use reinhardt::query::SimpleExpr;
