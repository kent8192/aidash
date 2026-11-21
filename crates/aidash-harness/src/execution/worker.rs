//! Revoked scoped cancellation precedes fresh worker admission and agent effects.
use aidash_application::{
	Result,
	ports::execution::worker::{Admission, WorkerLeases, WorkerStep},
};
use aidash_domain::{RunControl, capabilities::sessions::QUEUED};
use uuid::Uuid;

pub async fn advance(scope: &mut dyn WorkerStep, token: Uuid) -> Result<()> {
	if scope.cancel_scoped(token).await? {
		return Ok(());
	}
	if let Admission::Queued = scope.admit().await? {
		let store = scope.recovery_store();
		if let Some(mut run) = store.leased_run(token).await? {
			run.error = Some(QUEUED.into());
			store.save(&run, token, QUEUED).await?;
		}
		return Ok(());
	}
	let result = scope.invoke(token).await;
	scope.finish_authority(result).await
}

pub async fn cancellation_requested(leases: &dyn WorkerLeases, run: Uuid) -> Result<bool> {
	Ok(leases.control(run).await? == RunControl::Cancelled)
}

#[cfg(test)]
mod tests;
