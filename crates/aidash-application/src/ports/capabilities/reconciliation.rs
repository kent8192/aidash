//! Each reconciliation scope owns one native authority transaction until finish.
use crate::Result;
use aidash_domain::{
	RunMetadata,
	capabilities::{
		CoreCapabilities,
		operations::{
			MountedFile,
			reconciliation::{Area, Limits, Snapshot},
		},
	},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
pub struct Loaded {
	pub run: RunMetadata,
	pub area: Area,
	pub operation: Snapshot,
}
#[async_trait]
pub trait OperationReconciliationRepository: Send + Sync {
	async fn snapshot(&self, id: Uuid) -> Result<Snapshot>;
	async fn begin(
		&self,
		snapshot: &Snapshot,
	) -> Result<Box<dyn OperationReconciliationScope + '_>>;
	async fn withdraw(&self, snapshot: &Snapshot) -> Result<()>;
}
#[async_trait]
pub trait OperationReconciliationScope: Send {
	fn limits(&self) -> Limits;
	async fn load(&mut self, id: Uuid, run: Uuid) -> Result<Loaded>;
	async fn configuration(&mut self) -> Result<CoreCapabilities>;
	async fn require_builtin(&mut self, kind: &str) -> Result<()>;
	async fn authorize_packages(&mut self, request: &Value) -> Result<()>;
	async fn set_area(&mut self, state: &str) -> Result<()>;
	async fn complete_python(&mut self, operation: &Snapshot, observed: &Value) -> Result<()>;
	async fn persist(&mut self, operation: &Snapshot) -> Result<()>;
	async fn health(&mut self, cancelling: bool, python: bool) -> Result<Value>;
	async fn request(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value>;
	async fn dispatch_files(&mut self, operation: &Snapshot) -> Result<Vec<MountedFile>>;
	async fn upload_files(&mut self, operation: &Snapshot) -> Result<Vec<MountedFile>>;
	async fn read_chunk(&mut self, file: &MountedFile, offset: u64) -> Result<Vec<u8>>;
	async fn verified(&mut self, file: &MountedFile) -> Result<()>;
	async fn begin_output(&mut self, size: u64) -> Result<()>;
	async fn write_output(&mut self, bytes: &[u8]) -> Result<()>;
	async fn finish_output(&mut self, expected: &str) -> Result<(Uuid, String)>;
	async fn put(&mut self, kind: &str, bytes: &[u8]) -> Result<(Uuid, String)>;
	async fn publish(&mut self, entries: Vec<MountedFile>) -> Result<i64>;
	async fn event(&mut self, data: Value) -> Result<()>;
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()>;
}
