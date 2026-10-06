//! Transfer supervision drains each selected batch before stopping.
use aidash_application::{
	Result, capabilities::transfer, ports::capabilities::transfer::TransferRepository,
};
use std::time::Duration;
use tokio::sync::watch;
pub async fn run(
	repository: &dyn TransferRepository,
	mut stopping: watch::Receiver<bool>,
) -> Result<()> {
	loop {
		if *stopping.borrow() {
			return Ok(());
		};
		transfer::sweep(repository).await?;
		tokio::select! {_=stopping.changed()=>{},_=tokio::time::sleep(Duration::from_millis(300))=>{}}
	}
}
