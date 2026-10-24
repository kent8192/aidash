//! Durable cancellation retry slots and bounded foreign lifecycle reconciliation.
use crate::{Error, Result, ports::generation::provisioning::GenerationTerminalSession};
use aidash_domain::{RunPhase, generation::requests::Request};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait ForeignGenerationVisibility: Send {
	async fn suspend(&mut self) -> Result<()>;
}
#[async_trait]
pub trait ForeignGenerationMaintenance: Send + Sync {
	fn now(&self) -> DateTime<Utc>;
	async fn begin_visibility(&self) -> Result<Box<dyn ForeignGenerationVisibility>>;
	/// Claim the original database-time, thirty-second retry slot atomically.
	async fn reserve_retry(&self, id: Uuid) -> Result<u64>;
	async fn send_cancel(&self, target: &str, id: Uuid) -> Result<bool>;
	async fn mark_delivered(&self, id: Uuid) -> Result<()>;
	async fn pending(&self) -> Result<Vec<(Uuid, Value)>>;
	async fn jobs(&self) -> Result<Vec<Request>>;
	async fn cancel_job(&self, source: &str, id: Uuid) -> Result<Option<Request>>;
	async fn run_phase(&self, id: Uuid) -> Result<RunPhase>;
	/// Acquire the policy writer lock before reloading the request under its lock.
	async fn begin_terminal(&self, job: &Request) -> Result<Box<dyn GenerationTerminalSession>>;
	fn warn_cancel(&self, id: Uuid, error: &Error, retry: bool);
	fn notify(&self);
}
