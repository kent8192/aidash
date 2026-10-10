use super::{Manifest, Status, Vote};
use crate::{Result, federation::Federation};
use uuid::Uuid;

pub async fn status(f: &Federation, id: Uuid) -> Result<Status> {
	crate::bootstrap::transaction_coordinator(f)
		.status(id)
		.await
		.map(Into::into)
		.map_err(Into::into)
}
pub async fn votes(f: &Federation, id: Uuid) -> Result<Vec<Vote>> {
	crate::bootstrap::transaction_coordinator(f)
		.votes(id)
		.await
		.map(|votes| votes.into_iter().map(Into::into).collect())
		.map_err(Into::into)
}
pub async fn submit(f: &Federation, manifest: &Manifest) -> Result<Status> {
	submit_bound(f, manifest, None).await
}
pub(super) async fn submit_bound(
	f: &Federation,
	manifest: &Manifest,
	origin: Option<&super::authority::Origin>,
) -> Result<Status> {
	let origin = origin.map(Into::into);
	aidash_application::transactions::admission::submit(
		&crate::bootstrap::transaction_admission_repository(f),
		&crate::bootstrap::registry_validation_for(&f.store),
		manifest,
		origin.as_ref(),
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}
pub(crate) async fn submit_in(
	f: &Federation,
	manifest: &Manifest,
	origin: Option<&super::authority::Origin>,
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
) -> Result<Status> {
	let origin = origin.map(Into::into);
	aidash_application::transactions::admission::submit_in(
		&mut crate::bootstrap::transaction_admission_scope(f, tx),
		&crate::bootstrap::registry_validation_for(&f.store),
		manifest,
		origin.as_ref(),
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}

pub async fn abort(f: &Federation, id: Uuid) -> Result<Status> {
	crate::bootstrap::transaction_coordinator(f)
		.abort(id)
		.await
		.map(Into::into)
		.map_err(Into::into)
}
/// Make one durable protocol transition using the same application workflow as recovery.
pub async fn advance(f: &Federation, id: Uuid) -> Result<Status> {
	crate::bootstrap::transaction_coordinator(f)
		.advance(id)
		.await
		.map(Into::into)
		.map_err(Into::into)
}
pub async fn recover_once(f: &Federation) -> Result<()> {
	crate::bootstrap::transaction_coordinator(f)
		.recover_once()
		.await
		.map_err(Into::into)
}
pub async fn run(f: Federation) -> Result<()> {
	let (active, aborted) = crate::bootstrap::transaction_recovery_coordinators(&f).await?;
	aidash_runtime::transactions::run_coordinator(active, aborted)
		.await
		.map_err(Into::into)
}
