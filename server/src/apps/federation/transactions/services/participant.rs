//! Participant adapters share the application workflow and native persistence scope.
use super::{LocalStatus, Manifest};
use crate::{Result, federation::Federation};

pub async fn reserve(f: &Federation, caller: &str, manifest: &Manifest) -> Result<LocalStatus> {
	crate::bootstrap::transaction_participant(f)
		.reserve(caller, manifest)
		.await
		.map(Into::into)
		.map_err(Into::into)
}
pub async fn prepare(f: &Federation, caller: &str, manifest: &Manifest) -> Result<LocalStatus> {
	crate::bootstrap::transaction_participant(f)
		.prepare(caller, manifest)
		.await
		.map(Into::into)
		.map_err(Into::into)
}
pub async fn finish(f: &Federation, caller: &str, manifest: &Manifest) -> Result<LocalStatus> {
	crate::bootstrap::transaction_participant(f)
		.finish(caller, manifest)
		.await
		.map(Into::into)
		.map_err(Into::into)
}
pub async fn recover_once(f: &Federation) -> Result<usize> {
	crate::bootstrap::transaction_participant(f)
		.recover_once()
		.await
		.map_err(Into::into)
}

pub async fn run(f: Federation) -> Result<()> {
	aidash_runtime::transactions::run_participant(crate::bootstrap::transaction_participant(&f))
		.await
		.map_err(Into::into)
}
