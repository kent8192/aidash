//! Retained-object maintenance drains its accepted batch before stopping.
use aidash_application::{
	Result,
	capabilities::reclamation::{self, Cursor},
	ports::capabilities::reclamation::ReclamationRepository,
};
use std::time::Duration;
use tokio::sync::watch;
pub async fn run(
	repository: &dyn ReclamationRepository,
	mut stopping: watch::Receiver<bool>,
) -> Result<()> {
	let mut cursor = Cursor::default();
	loop {
		if *stopping.borrow() {
			return Ok(());
		}
		reclamation::sweep(repository, &mut cursor).await?;
		tokio::select! {_=stopping.changed()=>{},_=tokio::time::sleep(Duration::from_secs(1))=>{}}
	}
}
