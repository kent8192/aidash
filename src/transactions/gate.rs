//! Node-wide durable visibility and serialization barrier.
use crate::{Error, Result, store::Store};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub struct ReadLease {
	_transaction: Transaction<'static, Postgres>,
}
impl ReadLease {
	pub async fn begin(store: &Store) -> Result<Self> {
		let mut tx = store.control_pool.begin().await?;
		let pending: Option<Uuid> = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("transaction_id")),
				))
				.from(sea_orm::sea_query::Alias::new("atomic_gate"))
				.and_where(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("singleton")),
				))
				.lock_with_behavior(
					sea_orm::sea_query::LockType::Share,
					sea_orm::sea_query::LockBehavior::Nowait,
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.fetch_one(&mut *tx)
		.await
		.map_err(lock_error)?;
		if pending.is_some() {
			return Err(Error::TransactionPending);
		}
		Ok(Self { _transaction: tx })
	}
}
pub(crate) fn lock_error(error: sqlx::Error) -> Error {
	if error
		.as_database_error()
		.is_some_and(|e| e.code().as_deref() == Some("55P03"))
	{
		Error::TransactionPending
	} else {
		Error::Database(error)
	}
}
pub(crate) async fn exclusive(tx: &mut Transaction<'_, Postgres>) -> Result<Option<Uuid>> {
	sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("transaction_id")),
			))
			.from(sea_orm::sea_query::Alias::new("atomic_gate"))
			.and_where(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("singleton")),
			))
			.lock(sea_orm::sea_query::LockType::Update)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&mut **tx)
	.await
	.map_err(lock_error)
}
