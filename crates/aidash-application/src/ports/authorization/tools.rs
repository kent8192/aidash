//! A tool authority lease retains current policy, credentials, and its transaction.
use crate::Result;
use aidash_domain::{
	HumanRequest, RunMetadata, Task,
	policy::Resource,
	registry::{EntityRef, Entry, bindings::BindingSnapshot},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait AgentToolRepository: Send + Sync {
	fn is_remote(&self) -> bool;
	/// The admitted Binding snapshot of the Run being authorized.
	fn binding_snapshot(&self) -> Option<&BindingSnapshot>;
	async fn lease(&self) -> Result<Box<dyn AgentToolScope + '_>>;
}
#[async_trait]
pub trait AgentToolScope: Send {
	fn transaction_active(&self) -> bool;
	async fn suspend(&mut self) -> Result<()>;
	/// Replace the retained authority without releasing this scope's lease.
	async fn replace_remote_authority(&mut self) -> Result<bool>;
	fn context(&self) -> &Value;
	fn subjects(&self) -> &[String];
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn catalog(&mut self, reference: &EntityRef, action: &str) -> Result<Entry>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn task_read(&mut self, task: Uuid) -> Result<Task>;
	async fn task_resource(&mut self, task: &Task) -> Result<Resource>;
	async fn artifact_creation_resource(&mut self, task: Uuid, creator: &str) -> Result<Resource>;
	async fn memory_resource(&mut self, run: &RunMetadata) -> Result<Resource>;

	async fn human_request(&mut self, run: &RunMetadata, id: Uuid) -> Result<Option<HumanRequest>>;
	async fn human_resource(&mut self, request: &HumanRequest) -> Result<Resource>;
}
