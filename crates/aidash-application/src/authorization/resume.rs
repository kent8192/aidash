//! Worker resume rechecks current execution before accepting provider output.
use crate::{Result, ports::authorization::resume::WorkerResumeRepository};
use std::time::Duration;
pub async fn resume(repository: &dyn WorkerResumeRepository) -> Result<()> {
	if repository.remote() {
		return repository.refresh_remote().await;
	}
	let mut delay = Duration::from_millis(250);
	loop {
		let mut scope = repository.lease().await?;
		let result = async {
			scope.refresh().await?;
			scope.guard().await?;
			scope.inference().await
		}
		.await;
		match result {
			Ok(()) => return Ok(()),
			Err(error) if scope.retryable(&error) => {
				tracing::warn!(%error,"retrying execution-boundary reacquisition after transient database error");
				scope.discard_failed_refresh().await;
				drop(scope);
				repository.wait(delay).await;
				delay = delay.saturating_mul(2).min(Duration::from_secs(2));
			}
			Err(error) => return Err(error),
		}
	}
}
#[cfg(test)]
mod tests;
