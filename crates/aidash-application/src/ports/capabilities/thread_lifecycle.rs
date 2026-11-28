//! Persistence and authority for Thread deletion.
use crate::Result;
use aidash_domain::policy::Resource;
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
pub trait ThreadScope: DeletionAuthority {
	fn principal(&self) -> &str;
	async fn serialize(&mut self) -> Result<()>;
	async fn thread(&mut self, id: Uuid) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn visible_thread(&mut self, thread: Uuid) -> Result<()>;
	async fn record_tombstone(&mut self, thread: Uuid) -> Result<()>;
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>>;
	async fn cache(&mut self, key: Uuid, digest: &str, result: &Value) -> Result<()>;
	/// Lock the Thread row, so a Run admitted to it cannot commit behind
	/// its deletion.
	async fn lock_for_deletion(&mut self, thread: Uuid) -> Result<()>;
	/// Mark the locked Thread deleted: every read and write of it refuses
	/// from this transaction on.
	async fn mark_deleted(&mut self, thread: Uuid) -> Result<()>;
	/// Remove the deleted Thread's attachments in the tombstone transaction.
	async fn delete_attachments(&mut self, thread: Uuid) -> Result<()>;
	/// Stop the Duties whose own Thread this is (#557, #1011).
	async fn stop_schedules(&mut self, _thread: Uuid) -> Result<()> {
		Ok(())
	}
	/// Stop all work in the deleted Thread, whoever it belongs to (#1008):
	/// cancel every execution that has not ended and end every open Task.
	async fn stop_work(&mut self, thread: Uuid) -> Result<()>;
	async fn event(&mut self, thread: Uuid, kind: &str, data: Value) -> Result<()>;
}
