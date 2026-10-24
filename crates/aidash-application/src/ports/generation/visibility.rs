//! Generation visibility runs under the caller's borrowed authority lease.
use crate::Result;
use aidash_domain::{entities::Task, policy::Resource};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait GenerationVisibility: Send {
	fn inherited_lease(&self) -> bool;
	fn context(&mut self, value: Value);
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn task(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Task>>;
	async fn task_visible(&mut self, task: &Task) -> Result<bool>;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
}
