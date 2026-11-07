//! Capability process work owns every loop and waits for an in-flight batch before stopping.
use aidash_application::{
	Result,
	capabilities::{
		processing::{self, ReceiptCursor},
		read_recheck::{self, Schedule},
	},
	ports::capabilities::{
		processing::{CapabilityBackgroundJobs, OperationProcessingRepository},
		read_recheck::ReadRecheckRepository,
	},
};
use std::time::{Duration, Instant};
use tokio::sync::watch;
pub async fn run(
	repository: &dyn OperationProcessingRepository,
	rechecks: &dyn ReadRecheckRepository,
	jobs: &dyn CapabilityBackgroundJobs,
	stopping: watch::Receiver<bool>,
) -> Result<()> {
	tokio::try_join!(
		Box::pin(execution_loop(repository, stopping.clone())),
		Box::pin(read_recheck_loop(rechecks, stopping.clone())),
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
	loop {
		if *stopping.borrow() {
			return Ok(());
		}
		processing::sweep(repository, &mut cursor).await?;
		tokio::select! {_=stopping.changed()=>{},_=tokio::time::sleep(Duration::from_millis(300))=>{}}
	}
}
/// Apart from the operation sweep, so a slow check delays no other command.
async fn read_recheck_loop(
	repository: &dyn ReadRecheckRepository,
	mut stopping: watch::Receiver<bool>,
) -> Result<()> {
	let mut schedule = Schedule::default();
	loop {
		if *stopping.borrow() {
			return Ok(());
		}
		if let Err(error) = read_recheck::pass(repository, &mut schedule, Instant::now()).await {
			tracing::warn!(%error, "read recheck pass pending");
		}
		tokio::select! {_=stopping.changed()=>{},_=tokio::time::sleep(Duration::from_secs(1))=>{}}
	}
}
#[cfg(test)]
mod tests;
