//! Node-wide durable visibility and serialization barrier.
use crate::{Error, Result, store::Store};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub struct ReadLease {
	transaction: Option<Transaction<'static, Postgres>>,
	commit_epoch: i64,
}
impl ReadLease {
	pub async fn begin(store: &Store) -> Result<Self> {
		let mut tx = store.control_pool.begin().await?;
		let (pending, commit_epoch): (Option<Uuid>, i64) = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.columns([
					sea_orm::sea_query::Alias::new("transaction_id"),
					sea_orm::sea_query::Alias::new("commit_epoch"),
				])
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
		Ok(Self {
			transaction: Some(tx),
			commit_epoch,
		})
	}

	pub async fn suspend(&mut self) -> Result<()> {
		if let Some(transaction) = self.transaction.take() {
			transaction.rollback().await?;
		}
		Ok(())
	}

	pub async fn resume(&mut self, store: &Store) -> Result<()> {
		if self.transaction.is_none() {
			let mut delay = std::time::Duration::from_millis(250);
			loop {
				match Self::begin(store).await {
					Ok(lease) => {
						let stale = lease.commit_epoch != self.commit_epoch;
						*self = lease;
						if stale {
							return Err(Error::StaleInference);
						}
						break;
					}
					Err(Error::TransactionPending) => {
						tokio::time::sleep(std::time::Duration::from_millis(250)).await;
					}
					Err(error) if error.is_transient_database() => {
						tracing::warn!(%error, "retrying visibility lease reacquisition after transient database error");
						tokio::time::sleep(delay).await;
						delay = delay
							.saturating_mul(2)
							.min(std::time::Duration::from_secs(2));
					}
					Err(error) => return Err(error),
				}
			}
		}
		Ok(())
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
