//! A tool authority lease retains current policy, credentials, and its transaction.
use crate::Result;
use aidash_domain::{
	HumanRequest, RunMetadata, Task,
	agent_plugin::PluginTools,
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
	/// The plugin tools the Run pinned at its first step; `None` before it pinned any.
	fn plugin_tools(&self) -> Option<&PluginTools>;
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
	/// Admit a Registry entry for the call of `tool`; a policy denial names that tool.
	async fn catalog(&mut self, reference: &EntityRef, action: &str, tool: &str) -> Result<Entry>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	/// Whether the policy explicitly denies `action` on `resource` for any
	/// subject of the current authority. A missing allow is not a denial.
	async fn explicitly_denied(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	async fn task_read(&mut self, task: Uuid) -> Result<Task>;
	async fn task_resource(&mut self, task: &Task) -> Result<Resource>;
	async fn artifact_creation_resource(&mut self, task: Uuid, creator: &str) -> Result<Resource>;
	async fn memory_resource(&mut self, run: &RunMetadata) -> Result<Resource>;

	async fn human_request(&mut self, run: &RunMetadata, id: Uuid) -> Result<Option<HumanRequest>>;
	async fn human_resource(&mut self, request: &HumanRequest) -> Result<Resource>;
}
