//! Maintenance scopes own the existing native transaction and object ownership fences.
use crate::Result;
use aidash_domain::capabilities::records::Record;
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait ReclamationRepository: Send + Sync {
	async fn begin(&self) -> Result<Box<dyn ReclamationScope + '_>>;
	async fn retained(&self, after: Uuid) -> Result<Vec<Uuid>>;
	async fn working(&self, after: Uuid) -> Result<Vec<Uuid>>;
	async fn python(&self, cursor: &mut Uuid) -> Result<()>;
	async fn orphans(&self) -> Result<()>;
}
#[async_trait]
pub trait ReclamationScope: Send {
	async fn load(&mut self, id: Uuid) -> Result<Record>;
	async fn release_quota(&mut self, tenant: &str, reserved: i64) -> Result<()>;
	async fn object_kind(&mut self, id: Uuid, tenant: &str) -> Result<Option<String>>;
	async fn working_tenant(&mut self, id: Uuid) -> Result<Option<String>>;
	async fn erase(&mut self, tenant: &str, id: Uuid) -> Result<()>;
	async fn update(&mut self, record: &mut Record) -> Result<()>;
	async fn request(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value>;
	async fn finish(self: Box<Self>, result: Result<bool>) -> Result<()>;
}
