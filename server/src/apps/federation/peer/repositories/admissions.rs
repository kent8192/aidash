//! Receiver admission reads shared with other applications.
use crate::{Result, apps::federation::peer::models::AuthorizationRemoteAdmission};
use reinhardt::db::{backends::TransactionExecutor, orm::Model};
use uuid::Uuid;

/// The admission pinned for a Run, within the caller's Tenant transaction.
pub(crate) async fn for_run(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
) -> Result<Option<AuthorizationRemoteAdmission>> {
	let admission = AuthorizationRemoteAdmission::objects()
		.filter(AuthorizationRemoteAdmission::field_id().eq(run))
		.all_with_executor(tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop();
	Ok(admission)
}
