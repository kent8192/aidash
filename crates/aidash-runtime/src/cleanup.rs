//! File lifecycle maintenance drains its owned batch before shutdown.
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
	loop {
		if *stopping.borrow() {
			return Ok(());
		}
		cleanup::sweep(repository, &mut cursor).await?;
		tokio::select! {_=stopping.changed()=>{},_=tokio::time::sleep(Duration::from_secs(1))=>{}}
	}
}
