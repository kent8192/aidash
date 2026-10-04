//! Original subject authority, source snapshots and durable receipt reconciliation are separate scopes.
use crate::Result;
use aidash_domain::{
	RunMetadata,
	capabilities::{operations::MountedFile, records::Record, sessions::Area},
	policy::Resource,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
pub struct Limits {
	pub admission: bool,
	pub share_files: usize,
	pub share_file_bytes: u64,
	pub share_bytes: u64,
	pub staging_seconds: u64,
	pub working_bytes: u64,
}
#[async_trait]
pub trait TransferScope: Send {
	fn limits(&self) -> Result<Limits>;
	fn node_id(&self) -> &str;
	fn tenant(&self) -> &str;
	fn principal(&self) -> &str;
	fn credential(&self) -> Uuid;
	fn subjects(&self) -> &[String];
	fn set_subjects(&mut self, subjects: Vec<String>);
	fn set_context(&mut self, attributes: Value);
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn authorize(&mut self, area: &Area, action: &str) -> Result<()>;
	async fn sources(&mut self, workspace: Uuid, constraints: &Value) -> Result<()>;
	async fn run(&mut self, id: Uuid) -> Result<RunMetadata>;
	async fn entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry>;
	async fn check_pinned(&mut self, entry: &Entry) -> Result<()>;
	async fn peer_enabled(&mut self, node: &str) -> Result<Option<bool>>;
	async fn record(&mut self, id: Uuid, kind: &str) -> Result<Record>;
	async fn insert(
		&mut self,
		id: Uuid,
		area: Option<Uuid>,
		kind: &str,
		state: &str,
		data: Value,
		expires: Option<DateTime<Utc>>,
	) -> Result<Record>;
	async fn update(&mut self, record: &mut Record) -> Result<()>;
	async fn copy_snapshot(&mut self, file: &MountedFile) -> Result<MountedFile>;
	async fn read_chunk(&mut self, file: &MountedFile, offset: u64) -> Result<Vec<u8>>;
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>>;
	async fn cache(&mut self, key: Uuid, digest: &str, result: &Value) -> Result<()>;
	/// Commit only a successful result; every other exit releases the same owned authority transaction.
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()>;
}
#[async_trait]
pub trait ReceiptScope: Send {
	async fn load(&mut self) -> Result<Record>;
	async fn update(&mut self, record: &mut Record) -> Result<()>;
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()>;
}
#[async_trait]
pub trait TransferRepository: Send + Sync {
	fn limits(&self) -> Limits;
	async fn snapshot(&self, id: Uuid) -> Result<Record>;
	async fn begin_sender(&self, record: &Record) -> Result<Box<dyn TransferScope + '_>>;
	async fn begin_receipt(&self, id: Uuid) -> Result<Box<dyn ReceiptScope + '_>>;
	async fn request(&self, node: &str, path: &str, body: &Value) -> Result<Value>;
	async fn jobs(&self) -> Result<Vec<Uuid>>;
	async fn record_failure(&self, id: Uuid, terminal: bool) -> Result<()>;
}
