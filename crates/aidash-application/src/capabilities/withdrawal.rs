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
/// A durable Node deletion owns this exact operation. Unlike authority withdrawal,
/// it must also stop terminal/uncertain writers without rewriting their outcome.
pub async fn stop_captured_writer(
	repository: &dyn OperationWithdrawalRepository,
	area: Uuid,
	operation: Uuid,
) -> Result<()> {
	let scope = repository.lock(area, operation).await?;
	let snapshot = scope.snapshot();
	if aidash_domain::capabilities::operations::withdrawal::safe_to_erase(
		&snapshot.state,
		&snapshot.result,
		snapshot.runner_instance.as_deref(),
	) {
		return Ok(());
	}
	if snapshot.never_dispatched() && snapshot.runner_instance.is_some() {
		return Err(crate::Error::Conflict(
			"prepared writer has an unexpected runner instance".into(),
		));
	}
	let stopped = if (!snapshot.active() && snapshot.result["termination_confirmed"] == true)
		|| (snapshot.never_dispatched() && snapshot.runner_instance.is_none())
	{
		true
	} else {
		let instance = snapshot.runner_instance.as_deref().ok_or_else(|| {
			crate::Error::Conflict("captured writer has no original runner instance".into())
		})?;
		repository
			.cancel_bound(snapshot.operation_id, &snapshot.digest, instance)
			.await?
	};
	let change = if snapshot.active() {
		snapshot.change(stopped)
	} else {
		snapshot.confirm_terminal_stop(stopped)
	};
	scope.commit(change).await
}
#[cfg(test)]
mod tests;
