//! Capability process work owns every loop and waits for an in-flight batch before stopping.
use crate::background::Passes;
use aidash_application::{
	Result,
	capabilities::processing::{self, ReceiptCursor},
	ports::capabilities::processing::{CapabilityBackgroundJobs, OperationProcessingRepository},
};
use std::time::Duration;
use tokio::sync::watch;
pub async fn run(
	repository: &dyn OperationProcessingRepository,
	jobs: &dyn CapabilityBackgroundJobs,
	stopping: watch::Receiver<bool>,
) -> Result<()> {
	tokio::try_join!(
		Box::pin(execution_loop(repository, stopping.clone())),
		Box::pin(jobs.network(stopping.clone())),
		Box::pin(jobs.references(stopping.clone())),
		Box::pin(jobs.cleanup(stopping.clone())),
		Box::pin(jobs.reclamation(stopping)),
	)?;
	Ok(())
}
async fn execution_loop(
	repository: &dyn OperationProcessingRepository,
	mut stopping: watch::Receiver<bool>,
) -> Result<()> {
	let mut cursor = ReceiptCursor::default();
	let mut passes = Passes::new("capabilities.execution", Duration::from_millis(300));
	while !*stopping.borrow() {
		passes.run(processing::sweep(repository, &mut cursor)).await;
		passes.pause(&mut stopping).await;
	}
	Ok(())
}
#[cfg(test)]
mod tests;
