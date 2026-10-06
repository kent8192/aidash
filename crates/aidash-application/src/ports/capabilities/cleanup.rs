//! Cleanup scopes retain area-before-record locks and all existing ownership transactions.
use crate::Result;
use aidash_domain::{
	capabilities::{operations::MountedFile, records::Record, sessions::Area},
	policy::{PolicyBundle, Resource},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
pub struct Limits {
	pub working_bytes: u64,
	pub recovery_seconds: u64,
}
pub struct Creation {
	pub id: Uuid,
	pub area: Option<Uuid>,
	pub kind: &'static str,
	pub state: &'static str,
	pub data: Value,
	pub expires: Option<DateTime<Utc>>,
}
#[async_trait]
pub trait CleanupScope: Send {
	fn principal(&self) -> &str;
	fn credential(&self) -> Uuid;
	fn bundle(&self) -> &PolicyBundle;
	fn limits(&self) -> Result<Limits>;
	fn set_context(&mut self, value: Value);
	fn resource(&self, kind: &str, id: Uuid, attributes: Value) -> Resource;
	async fn serialize(&mut self) -> Result<()>;
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn sources(&mut self, workspace: Uuid, constraints: &Value) -> Result<()>;
	async fn message(&mut self, workspace: Uuid, id: Uuid) -> Result<()>;
	async fn visible_thread(&mut self, thread: Uuid) -> Result<()>;
	async fn load_area(&mut self, id: Uuid) -> Result<Option<Area>>;
	async fn inventory(&mut self, cursor: Option<Uuid>) -> Result<Vec<Area>>;
	async fn inventory_recovery(&mut self, area: &Area) -> Result<Option<Record>>;
	async fn cleanup_operation(&mut self, area: &Area) -> Result<Option<Uuid>>;
	async fn recoverable(&mut self, area: &Area) -> Result<Option<Record>>;
	async fn record_area(&mut self, id: Uuid) -> Result<Option<Option<Uuid>>>;
	async fn thread_root(&mut self, area: &Area, thread: Uuid) -> Result<Option<Uuid>>;
	async fn occupied(&mut self, area: &Area, thread: Uuid) -> Result<Option<Uuid>>;
	async fn reattach_working(&mut self, area: &Area, file: Uuid) -> Result<()>;
	async fn load_record(&mut self, id: Uuid, kind: &str) -> Result<Record>;
	async fn create(&mut self, record: Creation) -> Result<Record>;
	async fn update(&mut self, record: &mut Record) -> Result<()>;
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>>;
	async fn cache(&mut self, key: Uuid, digest: &str, result: &Value) -> Result<()>;
	async fn release_python(&mut self, area: &Area, reason: &str) -> Result<()>;
	async fn copy_owned(
		&mut self,
		area: Uuid,
		kind: &str,
		file: &MountedFile,
	) -> Result<MountedFile>;
	async fn verified(&mut self, file: &MountedFile) -> Result<()>;
	async fn publish(&mut self, area: &mut Area) -> Result<()>;
	async fn persist(&mut self, area: &Area, restoration: bool) -> Result<()>;
	async fn cancel_runs(&mut self, area: &Area) -> Result<()>;
	async fn revoke_grants(&mut self, area: &Area) -> Result<()>;
}
#[async_trait]
pub trait CleanupRepository: Send + Sync {
	async fn jobs(&self, after: Uuid) -> Result<Vec<Record>>;
	async fn begin(&self) -> Result<Box<dyn ErasureScope + '_>>;
	async fn failure(&self, id: Uuid, area: Option<Uuid>) -> Result<()>;
}
#[async_trait]
pub trait ErasureScope: Send {
	async fn locked(&mut self, snapshot: &Record) -> Result<(Area, Record)>;
	async fn objects(&mut self, area: &Area) -> Result<Vec<Uuid>>;
	async fn erase(&mut self, tenant: &str, id: Uuid) -> Result<()>;
	async fn update_area(&mut self, area: &Area) -> Result<()>;
	async fn update_record(&mut self, record: &mut Record) -> Result<()>;
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()>;
	async fn finish(self: Box<Self>, result: Result<bool>) -> Result<()>;
}
