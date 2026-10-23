//! Node deletion persistence. Lifecycle callers hold `nodes:<tenant>` before
//! recording or restoring; row locks fence restore against the purge worker.
use crate::Result;
use crate::apps::identity::models::NodeDeletion;
use aidash_domain::nodes::deletion::{self, Actor};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;

impl NodeDeletion {
	pub(crate) fn deleted_by(&self) -> Result<Actor> {
		Ok(Actor::parse(&self.deleted_by_kind)?)
	}
	pub(crate) fn rule(&self) -> Result<deletion::Deletion> {
		Ok(deletion::Deletion {
			deleted_by: self.deleted_by()?,
			purge_after: self.purge_after,
			restored: self.restored_at.is_some(),
			purged: self.purged_at.is_some(),
		})
	}
}

/// The unresolved deletion, including a completed purge. Closing a deletion
/// records a restore; deleting again inserts a new row with a fresh identity.
pub(crate) async fn current_locked(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
) -> Result<Option<NodeDeletion>> {
	Ok(NodeDeletion::objects()
		.filter(NodeDeletion::field_tenant().eq(tenant.to_owned()))
		.filter(NodeDeletion::field_node().eq(node.to_owned()))
		.filter(NodeDeletion::field_restored_at().is_null())
		.select_for_update()
		.all_with_executor(tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop())
}
