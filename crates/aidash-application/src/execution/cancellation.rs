//! Scoped receiver cancellation does not depend on reaching the source control node.
use crate::{Result, ports::execution::cancellation::ScopedCancellationRepository};
use aidash_domain::{Run, RunControl};
use uuid::Uuid;
pub async fn cancel_if_scoped(
	repository: &dyn ScopedCancellationRepository,
	run: &Run,
	token: Uuid,
) -> Result<bool> {
	if run.control != RunControl::Cancelled {
		return Ok(false);
	}
	if repository.remote_grant(&run.metadata()).await? {
		repository
			.save_receiver(&run.cancelled_delivery(), token, "run.cancelled")
			.await?;
		return Ok(true);
	}
	if !repository.local_grant(&run.metadata()).await? {
		return Ok(false);
	}
	repository.cancel_local(run, token).await?;
	Ok(true)
}
#[cfg(test)]
mod tests;
