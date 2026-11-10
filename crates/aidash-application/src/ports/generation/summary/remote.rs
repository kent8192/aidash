//! Remote Summary authorization refreshes the Home lease before reading its binding.
use crate::Result;
use aidash_domain::Run;
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait RemoteSummaryAuthority: Send {
	async fn suspend(&mut self) -> Result<()>;
	async fn refresh(&mut self, run: &Run) -> Result<bool>;
	async fn description(&mut self, run: Uuid) -> Result<Value>;
}
