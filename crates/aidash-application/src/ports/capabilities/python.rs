//! Heap scopes preserve current authority and owned area-before-record maintenance transactions.
use crate::Result;
use aidash_domain::{RunMetadata, capabilities::records::Record};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
/// What stopping and settling a Run's former per-command Python session needs.
/// New cells run in the Execution Pod (#745); these sessions are only ended.
#[async_trait]
pub trait PythonScope: Send {
	async fn load(&mut self, area: Uuid) -> Result<Record>;
	async fn update(&mut self, record: &mut Record) -> Result<()>;
	/// Persist the exact session writer stop before its binding may be reset.
	async fn confirm_stopped(&mut self, record: &Record) -> Result<()>;
	async fn request(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value>;
}
pub struct HeapOperation {
	/// The exact operation has durable stop and receipt settlement evidence,
	/// or was withdrawn before dispatch. A running heap may reset only then.
	pub writer_settled: bool,
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
	/// Persist the exact session writer stop before its binding may be reset.
	async fn confirm_stopped(&mut self, record: &Record) -> Result<()>;
	async fn finish(self: Box<Self>, result: Result<bool>) -> Result<()>;
}
