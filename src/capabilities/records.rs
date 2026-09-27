use super::sessions;
use crate::{Error, Result, authorization::access::Access};
use chrono::{DateTime, Utc};
use sea_orm::sea_query::{Alias, Expr, LockType, PostgresQueryBuilder, Query};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Record {
	pub id: Uuid,
	pub tenant: String,
	pub owner: String,
	pub area_id: Option<Uuid>,
	pub kind: String,
	pub state: String,
	pub revision: i64,
	pub data: Value,
	pub expires_at: Option<DateTime<Utc>>,
}
pub(crate) async fn get(access: &mut Access, id: Uuid, kind: &str) -> Result<Record> {
	sqlx::query_as(
		&sessions::select("core_records")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$2")))
			.and_where(Expr::col(Alias::new("kind")).eq(Expr::cust("$3")))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(&access.identity.tenant)
	.bind(kind)
	.fetch_optional(&mut **access.tx)
	.await?
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
			.values_panic((1..=8).map(|i| Expr::cust(format!("${i}"))))
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
	let changed = sqlx::query(
		&Query::update()
			.table(Alias::new("core_records"))
			.value(Alias::new("state"), Expr::cust("$2"))
			.value(Alias::new("data"), Expr::cust("$3"))
			.value(Alias::new("expires_at"), Expr::cust("$4"))
			.value(
				Alias::new("revision"),
				Expr::col(Alias::new("revision")).add(1),
			)
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("revision")).eq(Expr::cust("$5")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(record.id)
	.bind(&record.state)
	.bind(&record.data)
	.bind(record.expires_at)
	.bind(record.revision)
	.execute(&mut **tx)
	.await?
	.rows_affected();
	if changed != 1 {
		return Err(Error::Conflict("RECORD_CHANGED".into()));
	}
	record.revision += 1;
	Ok(())
}
