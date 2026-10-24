//! HTTP and worker entry points call the shared provisioning use cases.
use crate::{Result, authorization::access::Access, federation::Federation, registry::EntityRef};
use uuid::Uuid;

/// Resume durable generation work through the application-owned recovery flow.
pub async fn reconcile(f: &Federation) -> Result<usize> {
	aidash_application::generation::provisioning::reconcile(
		&crate::bootstrap::generation_provisioning_repository(f),
		&crate::bootstrap::registry_validation_for(&f.store),
	)
	.await
	.map_err(Into::into)
}

/// Retain the caller's inherited authority lease for every generation ancestor.
pub(crate) async fn require_live(
	access: &mut Access,
	node: &str,
	task: Uuid,
	agent: &EntityRef,
) -> Result<()> {
	aidash_application::generation::publication::require_live(
		&mut crate::bootstrap::generation_live_scope(access),
		node,
		task,
		agent,
	)
	.await
	.map_err(Into::into)
}
