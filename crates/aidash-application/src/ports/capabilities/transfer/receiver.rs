//! Receiver scopes keep identity mapping, reservation and publication in one owned transaction.
use super::{TransferRepository, TransferScope};
use crate::Result;
use aidash_domain::capabilities::{
	operations::MountedFile, records::Record, sessions::Area, transfer::Description,
};
use aidash_domain::policy::Resource;
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait ReceiverScope: TransferScope {
	/// The owned receiving Node with the name and attributes used by its policy.
	async fn node_resource(&mut self) -> Result<Resource>;
	async fn recipient(&mut self, description: &Description, node_id: &str)
	-> Result<Option<Area>>;
	async fn admitted(&mut self, area: &Area, version: &str) -> Result<Option<Uuid>>;
	async fn recipient_rows(&mut self, cursor: Option<Uuid>) -> Result<Vec<Area>>;
	async fn versions(&mut self, area: &Area) -> Result<Vec<String>>;
	async fn reserve(&mut self, bytes: i64) -> Result<()>;
	/// Fence reservation creation and conversion against other transfers of this Tenant.
	async fn lock_conversion(&mut self) -> Result<()>;
	async fn put_staging(&mut self, bytes: &[u8]) -> Result<(Uuid, String)>;
	async fn begin_received(&mut self, area: Uuid, size: u64, credit: i64) -> Result<()>;
	async fn write_pending(&mut self, bytes: &[u8]) -> Result<()>;
	async fn finish_pending(&mut self, digest: &str) -> Result<(Uuid, String)>;
	async fn read_file(&mut self, file: &MountedFile) -> Result<Vec<u8>>;
	async fn publish(&mut self, area: &mut Area) -> Result<()>;
	async fn event(&mut self, workspace: Uuid, kind: &str, data: serde_json::Value) -> Result<()>;
}
/// A Home taking a delivery from a Run executing for another Home (ADR 0139, decision 20). The
/// delivery's own admission (the live Remote Grant, its Binding and the tool the Run called)
/// stands in for the Node-level `file.transfer` a Peer Mapping must hold, so `mapped` does not
/// ask for it; the delegating Run's Area is still authorized for `file.receive`.
#[derive(Clone, Copy)]
pub struct DeliveryMode {
	/// The tool the delegating Run's own Binding must hold for the Area to take the files, or
	/// `None` when the delivery is not a share into that Area.
	pub bound_operation: Option<&'static str>,
}
#[async_trait]
pub trait ReceiverRepository: TransferRepository {
	/// `Some` for a Home taking a delivery, `None` for a Peer Mapping's `file-transfer/1`.
	fn delivery(&self) -> Option<DeliveryMode> {
		None
	}
	fn node_id(&self) -> &str;
	async fn incoming(&self, id: Uuid) -> Result<Record>;
	async fn status_snapshot(&self, id: Uuid) -> Result<Record>;
	/// The mapped authority of `subject` of `source` at the receiving Node
	/// this repository was built for.
	/// A callback from the receiving Node to `source`.
	async fn callback(&self, source: &str, path: &str, body: &Value) -> Result<Value>;
	async fn begin_peer(&self, source: &str, subject: &str) -> Result<Box<dyn ReceiverScope + '_>>;
}
