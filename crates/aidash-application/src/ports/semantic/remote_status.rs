//! Summaries use current generation authority before loading origin-owned counters.
use crate::{Result, ports::generation::visibility::GenerationVisibility};
use aidash_domain::{
	generation::requests::Request,
	semantic::remote::{journal::Record, status::Allowance},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait StatusScope: GenerationVisibility {
	fn node_id(&self) -> &str;
	fn tenant(&self) -> &str;
	async fn request(&mut self, id: Uuid, tenant: &str) -> Result<Option<Request>>;
	async fn allowance(&mut self, id: Uuid) -> Result<Allowance>;
	async fn run_visible(&mut self, id: Uuid) -> Result<bool>;
	async fn receipt(&mut self, id: Uuid) -> Result<Option<Value>>;
}
#[async_trait]
pub trait StatusRepository: Send + Sync {
	async fn latest(&self, grant: Uuid) -> Result<Option<Record>>;
}
