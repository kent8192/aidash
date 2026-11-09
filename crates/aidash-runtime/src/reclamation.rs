//! Retained-object maintenance drains its accepted batch before stopping.
use crate::background::Passes;
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
	let mut passes = Passes::new("capabilities.reclamation", Duration::from_secs(1));
	while !*stopping.borrow() {
		passes
			.run(reclamation::sweep(repository, &mut cursor))
			.await;
		passes.pause(&mut stopping).await;
	}
	Ok(())
}
