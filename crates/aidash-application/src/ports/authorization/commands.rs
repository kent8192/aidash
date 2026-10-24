//! Command replay and authorization borrow the existing source authority lease.
use crate::Result;
use aidash_domain::{Task, identity::commands::Binding, policy::Resource};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait RemoteCommandScope: Send {
	async fn binding(&mut self, grant: Uuid) -> Result<Option<Binding>>;
	async fn task(&mut self, id: Uuid) -> Result<Task>;
	async fn previous(&mut self, grant: Uuid, key: &str) -> Result<Option<(String, Value)>>;
	async fn task_resource(&mut self, task: &Task) -> Result<Resource>;
	async fn require_builtin(&mut self, tool: &str) -> Result<()>;
}

pub mod effects;
