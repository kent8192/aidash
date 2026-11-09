//! File lifecycle maintenance drains its owned batch before shutdown.
use crate::background::Passes;
use aidash_application::{
	Result, capabilities::cleanup, ports::capabilities::cleanup::CleanupRepository,
};
use std::time::Duration;
use tokio::sync::watch;
use uuid::Uuid;
pub async fn run(
	repository: &dyn CleanupRepository,
	mut stopping: watch::Receiver<bool>,
) -> Result<()> {
	let mut cursor = Uuid::nil();
	let mut passes = Passes::new("capabilities.cleanup", Duration::from_secs(1));
	while !*stopping.borrow() {
		passes.run(cleanup::sweep(repository, &mut cursor)).await;
		passes.pause(&mut stopping).await;
	}
	Ok(())
}
