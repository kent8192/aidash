//! Participant adapters share the application workflow and native persistence scope.
use super::{LocalStatus, Manifest};
use crate::{Result, federation::Federation};
use aidash_application::federation::inbound::Admitted;

fn participant(
	f: &Federation,
	admitted: Option<&Admitted>,
) -> aidash_application::transactions::participation::Participant {
	match admitted {
		Some(admitted) => crate::bootstrap::transaction_participant_as(f, admitted),
		None => crate::bootstrap::transaction_participant(f),
	}
}

pub async fn reserve(
	f: &Federation,
	admitted: Option<&Admitted>,
	caller: &str,
	manifest: &Manifest,
) -> Result<LocalStatus> {
	crate::apps::federation::transactions::repositories::participation::require_live_participant(
		f, manifest,
	)
	.await?;
	participant(f, admitted)
		.reserve(caller, manifest)
		.await
		.map(Into::into)
		.map_err(Into::into)
}
pub async fn prepare(
	f: &Federation,
	admitted: Option<&Admitted>,
	caller: &str,
	manifest: &Manifest,
) -> Result<LocalStatus> {
	crate::apps::federation::transactions::repositories::participation::require_live_participant(
		f, manifest,
	)
	.await?;
	participant(f, admitted)
		.prepare(caller, manifest)
		.await
		.map(Into::into)
		.map_err(Into::into)
}
pub async fn finish(
	f: &Federation,
	admitted: Option<&Admitted>,
	caller: &str,
	manifest: &Manifest,
) -> Result<LocalStatus> {
	participant(f, admitted)
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
