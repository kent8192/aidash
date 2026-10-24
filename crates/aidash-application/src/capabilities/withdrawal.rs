//! Keep cancellation possible after credential or source authority is revoked.
use crate::{Result, ports::capabilities::withdrawal::OperationWithdrawalRepository};
use uuid::Uuid;
pub async fn withdraw(
	repository: &dyn OperationWithdrawalRepository,
	area: Uuid,
	operation: Uuid,
) -> Result<()> {
	let scope = repository.lock(area, operation).await?;
	let snapshot = scope.snapshot();
	if !snapshot.active() {
		return Ok(());
	}
	let stopped = if snapshot.never_dispatched() {
		true
	} else {
		repository.cancel(snapshot.operation_id).await?["termination_confirmed"] == true
	};
	scope.commit(snapshot.change(stopped)).await
}
#[cfg(test)]
mod tests;
