//! Reference operations borrow HTTP authority or own the same worker transaction.
use crate::Result;
use aidash_domain::capabilities::{operations::MountedFile, records::Record};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
pub struct Limits {
	pub admission: bool,
	pub reference_bytes: u64,
	pub reference_files: usize,
	pub reference_text_bytes: usize,
	pub reference_pages: usize,
	pub working_bytes: u64,
	pub output_bytes: u64,
	pub operation_seconds: u64,
	pub staging_seconds: u64,
	pub runner_image: Option<String>,
}
pub struct Mounts {
	pub manifest: Value,
	pub constraints: Value,
}
#[async_trait]
pub trait ReferenceScope: Send {
	fn principal(&self) -> &str;
	fn identity(&self) -> Value;
	fn limits(&self) -> Result<Limits>;
	fn mounts(&self) -> Result<Mounts>;
	fn set_constraints(&mut self, value: Value) -> Result<()>;
	fn set_manifest(&mut self, value: Value) -> Result<()>;
	async fn load(&mut self, id: Uuid) -> Result<Record>;
	async fn require(&mut self, id: Uuid, owner: &str, action: &str) -> Result<()>;
	async fn require_upload(&mut self) -> Result<()>;
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>>;
	async fn cache(&mut self, key: Uuid, digest: &str, value: &Value) -> Result<()>;
	async fn insert(
		&mut self,
		id: Uuid,
		state: &str,
		data: Value,
		expires: Option<DateTime<Utc>>,
	) -> Result<Record>;
	async fn update(&mut self, record: &mut Record) -> Result<()>;
	async fn put(&mut self, kind: &str, bytes: &[u8]) -> Result<(Uuid, String)>;
	async fn read(&mut self, file: &MountedFile) -> Result<Vec<u8>>;
	async fn read_chunk(&mut self, file: &MountedFile, offset: u64) -> Result<Vec<u8>>;
	async fn begin_original(&mut self, size: u64) -> Result<()>;
	async fn write_original(&mut self, bytes: &[u8]) -> Result<()>;
	async fn finish_original(&mut self, digest: &str) -> Result<(Uuid, String)>;
	async fn release_python(&mut self, reason: &str) -> Result<()>;
	async fn publish(&mut self) -> Result<()>;
	async fn verified_health(&mut self) -> Result<Value>;
	async fn request(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value>;
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()>;
}
#[async_trait]
pub trait ReferenceRepository: Send + Sync {
	async fn snapshot(&self, id: Uuid) -> Result<Record>;
	async fn begin(&self, record: &Record) -> Result<Box<dyn ReferenceScope + '_>>;
	async fn active(&self, after: Uuid) -> Result<Vec<Uuid>>;
	async fn receipts(&self, after: Uuid) -> Result<Vec<(Uuid, Value)>>;
	async fn acknowledge(&self, id: Uuid) -> Result<()>;
	async fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value>;
}
