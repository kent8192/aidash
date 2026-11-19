//! Thread deletion extends the same cleanup transaction with ordered owned-area reads.
use super::cleanup::CleanupScope;
use crate::Result;
use aidash_domain::{capabilities::sessions::Area, policy::Resource};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
/// Who may delete a Thread (#1008): the decisions it needs.
#[async_trait]
pub trait DeletionAuthority: Send {
	/// Whether the Tenant's policy allows `action` on `resource`, recorded
	/// like every decision.
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	/// The Node the Thread belongs to, as a `node` resource.
	async fn node_resource(&mut self, thread: Uuid) -> Result<Resource>;
}
#[async_trait]
pub trait ThreadScope: CleanupScope + DeletionAuthority {
	/// Lock the Thread row, so a Run admitted to it cannot commit behind
	/// its deletion.
	async fn lock_for_deletion(&mut self, thread: Uuid) -> Result<()>;
	/// Mark the locked Thread deleted: every read and write of it refuses
	/// from this transaction on.
	async fn mark_deleted(&mut self, thread: Uuid) -> Result<()>;
	async fn owned_areas(&mut self, thread: Uuid, offset: u64) -> Result<Vec<Area>>;
	/// Remove the deleted Thread's attachments in the tombstone transaction.
	async fn delete_attachments(&mut self, thread: Uuid) -> Result<()>;
	async fn cancel_generation(&mut self, area: &Area) -> Result<()>;
	/// Stop the Duties whose own Thread this is (#557, #1011).
	async fn stop_schedules(&mut self, _thread: Uuid) -> Result<()> {
		Ok(())
	}
	async fn event(&mut self, thread: Uuid, kind: &str, data: Value) -> Result<()>;
}
