//! Capability loops. The server supervises each as its own named service; every
//! loop finishes its in-flight batch once stopping is signalled.
use crate::background::Passes;
use aidash_application::{
	Result,
	capabilities::{
		processing::{self, ReceiptCursor},
		read_recheck::{self, Schedule},
	},
	ports::capabilities::{
		processing::OperationProcessingRepository, read_recheck::ReadRecheckRepository,
	},
};
use std::time::{Duration, Instant};
use tokio::sync::watch;
pub async fn execution_loop(
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
/// Apart from the operation sweep, so a slow check delays no other command.
pub async fn read_recheck_loop(
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
