//! Every authority scope owns its transaction until durable admission or publication finishes.
use crate::Result;
use aidash_domain::capabilities::records::Record;
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
pub struct FetchPolicy {
	pub origins: Vec<String>,
	pub output_bytes: u64,
}
pub struct AuthorizedRun {
	pub id: Uuid,
	pub workspace_id: Uuid,
}
#[async_trait]
pub trait OutboundTransport: Send + Sync {
	async fn fetch(
		&self,
		url: &str,
		targets: &Value,
		policy: &FetchPolicy,
	) -> Result<(u16, Vec<u8>, String)>;
}
#[async_trait]
pub trait OutboundRepository: Send + Sync {
	fn fetch_policy(&self) -> FetchPolicy;
	async fn active_operations(&self) -> Result<Vec<Uuid>>;
	async fn snapshot(&self, id: Uuid) -> Result<Record>;
	async fn begin(&self, record: &Record) -> Result<Box<dyn OutboundScope + '_>>;
	async fn fail(&self, id: Uuid, message: &str) -> Result<()>;
}
#[async_trait]
pub trait OutboundScope: Send {
	async fn load(&mut self, id: Uuid) -> Result<Record>;
	async fn authorize(&mut self, record: &Record) -> Result<AuthorizedRun>;
	async fn update(&mut self, record: &mut Record) -> Result<()>;
	async fn put(&mut self, area: Option<Uuid>, bytes: &[u8]) -> Result<(Uuid, String)>;
	async fn event(&mut self, run: &AuthorizedRun, id: Uuid, status: u16) -> Result<()>;
	async fn finish(self: Box<Self>, result: Result<Option<Record>>) -> Result<Option<Record>>;
}
