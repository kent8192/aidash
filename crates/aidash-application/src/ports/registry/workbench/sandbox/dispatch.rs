//! Real-tool dispatch holds current session, draft, profile and identity leases through network I/O.
use super::SandboxScope;
use crate::Result;
use aidash_domain::{
	provider::ToolCall,
	registry::{
		EntityRef, Entry,
		workbench::profile::{RealToolRule, TestProfile},
	},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait RealDispatchScope: SandboxScope {
	async fn lock_identity(&mut self) -> Result<()>;
	async fn profile(&mut self, tenant: &str, id: &str) -> Result<TestProfile>;
	async fn effective(&mut self, reference: &EntityRef) -> Result<Entry>;
	async fn write_calls(&mut self, id: Uuid, calls: Value, running: bool) -> Result<()>;
	async fn rollback(self: Box<Self>) -> Result<()>;
}
#[async_trait]
pub trait RealDispatchRepository: Send + Sync {
	async fn begin_real(&self) -> Result<Box<dyn RealDispatchScope + '_>>;
}
#[async_trait]
pub trait PreparedRealRequest: Send {
	async fn send(self: Box<Self>) -> Result<(Value, &'static str)>;
}
pub trait RealToolTransport: Send + Sync {
	/// Build and validate without sending; preserve failures before the durable pending marker.
	fn prepare(
		&self,
		session: Uuid,
		rule: &RealToolRule,
		call: &ToolCall,
	) -> Result<Box<dyn PreparedRealRequest>>;
}
