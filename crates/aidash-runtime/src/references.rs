//! Reference extraction owns the cursor and completes an accepted batch before stopping.
use crate::background::Passes;
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
	let mut passes = Passes::new("capabilities.references", Duration::from_millis(500));
	while !*stopping.borrow() {
		passes.run(references::sweep(repository, &mut cursor)).await;
		passes.pause(&mut stopping).await;
	}
	Ok(())
}
