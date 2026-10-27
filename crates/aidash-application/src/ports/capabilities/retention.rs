//! Retention holds are written in the caller's authority transaction and scope.
use crate::Result;
use aidash_domain::{
	capabilities::{
		retention::{Holder, RetainedFile, RetentionHold},
		sessions::Area,
	},
	policy::Resource,
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

/// The Deployment ceilings a hold is admitted under.
pub struct RetentionLimits {
	/// Bytes one Working Area's manifest and retained versions may hold.
	pub working_bytes: u64,
	/// Bytes the Tenant's objects may hold, retained versions included.
	pub retained_bytes: u64,
}

/// One authority-and-persistence transaction for retention holds.
///
/// The caller opens it with current authority, so every read, the quota check
/// and the insert observe and commit as one transaction. Implementations must
/// render the persisted hold and the object lock the insert relies on.
#[async_trait]
pub trait RetentionScope: Send {
	fn tenant(&self) -> &str;
	fn principal(&self) -> &str;
	fn limits(&self) -> Result<RetentionLimits>;
	fn set_context(&mut self, context: Value);
	fn resource(&self, kind: &str, id: Uuid, attributes: Value) -> Resource;
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	/// The Working Area in the caller's Tenant, or `None`.
	async fn area(&mut self, id: Uuid) -> Result<Option<Area>>;
	/// Every hold of the Working Area, with the File versions it keeps.
	async fn holds(&mut self, area: Uuid) -> Result<Vec<RetentionHold>>;
	/// The Tenant's committed object bytes, retained versions included.
	async fn used_bytes(&mut self) -> Result<u64>;
	/// Bytes of superseded objects that no active hold keeps.
	async fn reclaimable_bytes(&mut self) -> Result<u64>;
	async fn insert(&mut self, hold: &RetentionHold) -> Result<()>;
	/// Deletes every hold of `holder` and returns how many rows it removed.
	async fn release(&mut self, holder: &Holder) -> Result<u64>;
	/// The File versions the active hold covering `file_id` recorded, or `None`.
	async fn manifest_of(&mut self, area: Uuid, file_id: Uuid)
	-> Result<Option<Vec<RetainedFile>>>;
}
