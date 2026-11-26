//! Revoked scoped cancellation precedes fresh worker admission and agent effects.
use aidash_application::{
	Result,
	ports::execution::worker::{WorkerLeases, WorkerStep},
};
use aidash_domain::RunControl;
use uuid::Uuid;

pub async fn advance(scope: &mut dyn WorkerStep, token: Uuid) -> Result<()> {
	if scope.cancel_scoped(token).await? {
		return Ok(());
	}
	scope.admit().await?;
	let result = scope.invoke(token).await;
	scope.finish_authority(result).await
}

pub async fn cancellation_requested(leases: &dyn WorkerLeases, run: Uuid) -> Result<bool> {
	Ok(leases.control(run).await? == RunControl::Cancelled)
}

#[cfg(test)]
mod tests;
