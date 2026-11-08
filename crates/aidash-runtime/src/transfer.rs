//! Transfer supervision drains each selected batch before stopping.
use crate::background::Passes;
use aidash_application::{
	Result, capabilities::transfer, ports::capabilities::transfer::TransferRepository,
};
use std::time::Duration;
use tokio::sync::watch;
pub async fn run(
	repository: &dyn TransferRepository,
	mut stopping: watch::Receiver<bool>,
) -> Result<()> {
	let mut passes = Passes::new("capabilities.transfer", Duration::from_millis(300));
	while !*stopping.borrow() {
		passes.run(transfer::sweep(repository)).await;
		passes.pause(&mut stopping).await;
	}
	Ok(())
}
