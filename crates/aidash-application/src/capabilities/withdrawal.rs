//! Keep cancellation possible after credential or source authority is revoked.
use crate::{Result, ports::capabilities::withdrawal::OperationWithdrawalRepository};
use aidash_domain::capabilities::operations::withdrawal::WriterStop;
use uuid::Uuid;
/// End an operation whose authority is gone. A dispatched operation is marked
/// withdrawn, as `cancelling`, in a commit of its own before the Runner is asked
/// to stop it: Egress refuses a stream from that row, so a Runner that cannot be
/// reached leaves nothing running on the revoked authority. The marker releases
/// the locks, so the stop's outcome is applied under fresh ones to whatever the
/// row is by then.
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
	if snapshot.never_dispatched() {
		return scope.commit(snapshot.change(WriterStop::Confirmed)).await;
	}
	if snapshot.marked_withdrawn() {
		drop(scope);
	} else {
		scope
			.commit(snapshot.change(WriterStop::Unconfirmed))
			.await?;
	}
	let reply = repository
		.cancel(snapshot.operation_id, snapshot.undelivered_refusal())
		.await?;
	let stop = WriterStop::from_reply(&reply);
	if stop == WriterStop::Unconfirmed {
		// The marker already says so, and the sweep goes on asking the Runner.
		return Ok(());
	}
	let scope = repository.lock(area, operation).await?;
	let settled = scope.snapshot();
	if !settled.active() {
		// A pass of the sweep ended it while the Runner was being asked.
		return Ok(());
	}
	scope.commit(settled.change(stop)).await
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
	let stop = if (!snapshot.active() && snapshot.result["termination_confirmed"] == true)
		|| (snapshot.never_dispatched() && snapshot.runner_instance.is_none())
	{
		WriterStop::Confirmed
	} else {
		let instance = snapshot.runner_instance.as_deref().ok_or_else(|| {
			crate::Error::Conflict("captured writer has no original runner instance".into())
		})?;
		repository
			.cancel_bound(
				snapshot.operation_id,
				&snapshot.digest,
				instance,
				snapshot.undelivered_refusal(),
			)
			.await?
	};
	let change = if snapshot.active() {
		snapshot.change(stop)
	} else {
		snapshot.confirm_terminal_stop(stop != WriterStop::Unconfirmed)
	};
	scope.commit(change).await
}
#[cfg(test)]
mod tests;
