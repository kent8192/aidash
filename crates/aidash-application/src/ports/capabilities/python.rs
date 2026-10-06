//! Heap scopes preserve current authority and owned area-before-record maintenance transactions.
use crate::Result;
use aidash_domain::{
	RunMetadata,
	capabilities::{operations::ShellRequest, records::Record, sessions::Area},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
pub struct Limits {
	pub admission: bool,
	pub command_bytes: usize,
	pub idle_seconds: u64,
	pub image: Option<String>,
}
#[async_trait]
pub trait PythonScope: Send {
	fn subjects(&self) -> &[String];
	fn credential(&self) -> Uuid;
	fn policy_revision(&self) -> i64;
	fn limits(&self) -> Result<Limits>;
	async fn load(&mut self, area: Uuid) -> Result<Record>;
	async fn create(&mut self, area: Uuid, data: Value) -> Result<Record>;
	async fn update(&mut self, record: &mut Record) -> Result<()>;
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>>;
	async fn cache(&mut self, key: Uuid, digest: &str, value: &Value) -> Result<()>;
	async fn previous_result(&mut self, id: Uuid) -> Result<Value>;
	async fn current_run(&mut self, area: &Area) -> Result<Option<Uuid>>;
	async fn require_write(&mut self, area: &Area) -> Result<()>;
	async fn health(&mut self) -> Result<Value>;
	async fn request(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value>;
	async fn environment(&mut self, area: &Area) -> Result<Value>;
	async fn prepare(
		&mut self,
		area: &mut Area,
		input: ShellRequest,
		extra: Value,
	) -> Result<Value>;
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()>;
}
pub struct HeapOperation {
	pub credential_id: Uuid,
	pub tenant: String,
	pub principal: String,
	pub subjects: Value,
	pub run_id: Uuid,
	pub policy_revision: i64,
	pub area_id: Uuid,
}
#[async_trait]
pub trait PythonRepository: Send + Sync {
	fn admission(&self) -> bool;
	fn idle_seconds(&self) -> u64;
	async fn frozen(&self, after: Uuid) -> Result<Vec<Record>>;
	async fn operation(&self, id: Uuid) -> Result<HeapOperation>;
	async fn authority(&self, operation: &HeapOperation) -> Result<Box<dyn HeapAuthority + '_>>;
	async fn begin(&self) -> Result<Box<dyn HeapMaintenance + '_>>;
}
#[async_trait]
pub trait HeapAuthority: Send {
	fn set_subjects(&mut self, subjects: Vec<String>);
	fn policy_revision(&self) -> i64;
	async fn interaction_run(&mut self, id: Uuid) -> Result<RunMetadata>;
	async fn context_authority(&mut self, run: &RunMetadata) -> Result<()>;
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()>;
}
#[async_trait]
pub trait HeapMaintenance: Send {
	async fn locked(&mut self, snapshot: &Record, operation: &HeapOperation) -> Result<Record>;
	async fn request(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value>;
	async fn update(&mut self, record: &mut Record) -> Result<()>;
	async fn finish(self: Box<Self>, result: Result<bool>) -> Result<()>;
}
