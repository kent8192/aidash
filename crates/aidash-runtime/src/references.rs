//! Reference extraction owns the cursor and completes an accepted batch before stopping.
use aidash_application::{
	Result,
	capabilities::references::{self, Cursor},
	ports::capabilities::references::ReferenceRepository,
};
use std::time::Duration;
use tokio::sync::watch;
pub async fn run(
	repository: &dyn ReferenceRepository,
	mut stopping: watch::Receiver<bool>,
) -> Result<()> {
	let mut cursor = Cursor::default();
	loop {
		if *stopping.borrow() {
			return Ok(());
		}
		references::sweep(repository, &mut cursor).await?;
		tokio::select! {_=stopping.changed()=>{},_=tokio::time::sleep(Duration::from_millis(500))=>{}}
	}
}
