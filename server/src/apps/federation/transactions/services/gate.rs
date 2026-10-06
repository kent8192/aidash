//! Node-wide durable visibility and serialization barrier.
use crate::apps::federation::transactions::models::AtomicGate;
use crate::{Error, Result, store::Store};
use reinhardt::db::backends::TransactionExecutor;

pub struct ReadLease {
	transaction: Option<Box<dyn TransactionExecutor>>,
	commit_epoch: i64,
}
impl ReadLease {
	pub async fn begin(store: &Store) -> Result<Self> {
		let connection = store.control_pool.connection();
		let (tx, commit_epoch) = AtomicGate::read_lease(&connection).await?;
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
pub(crate) fn lock_error(error: Error) -> Error {
	if error.has_database_code("55P03") {
		Error::TransactionPending
	} else {
		error
	}
}

/// Retain the visibility lock in an existing authority transaction.
pub(crate) async fn read_in(tx: &mut crate::database::native::Transaction) -> Result<i64> {
	let (pending, commit_epoch): (Option<Uuid>, i64) = crate::database::native::query_as(
		&reinhardt::query::Query::select()
			.columns([
				reinhardt::query::Alias::new("transaction_id"),
				reinhardt::query::Alias::new("commit_epoch"),
			])
			.from(reinhardt::query::Alias::new("atomic_gate"))
			.and_where(reinhardt::query::SimpleExpr::from(
				reinhardt::query::Expr::col(reinhardt::query::Alias::new("singleton")),
			))
			.lock(reinhardt::query::LockType::Share)
			.lock_behavior(reinhardt::query::LockBehavior::Nowait)
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.columns(&["transaction_id", "commit_epoch"])
	.fetch_one(&mut **tx)
	.await
	.map_err(lock_error)?;
	if pending.is_some() {
		return Err(Error::TransactionPending);
	}
	Ok(commit_epoch)
}

use reinhardt::query::QueryStatementBuilder as _;

use uuid::Uuid;
