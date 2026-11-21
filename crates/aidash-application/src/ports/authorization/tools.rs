//! A tool authority lease retains current policy, credentials, and its transaction.
use crate::Result;
use aidash_domain::{
	HumanRequest, RunMetadata, Task,
	agent_plugin::PluginTools,
	federation::tool_lists::{OwnedRemoteToolLists, ToolList},
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
	/// Whether the Home records `task` as an output of the Remote Grant that
	/// started this Run (#654): a Task the Run created there, which it may hand
	/// on. Asked only for a Run started by a Remote Grant, outside any lease.
	async fn grant_output(&self, task: Uuid) -> Result<bool>;
}
#[async_trait]
pub trait AgentToolScope: Send {
	/// None outside a Subnode; a dissolved Subnode or removed member is refused.
	async fn subnode_allowance(&mut self) -> Result<Option<aidash_domain::subnodes::Allowance>>;
	fn transaction_active(&self) -> bool;
	async fn suspend(&mut self) -> Result<()>;
	/// Replace the retained authority without releasing this scope's lease.
	async fn replace_remote_authority(&mut self) -> Result<bool>;
	/// The Federation tool lists of a remote Run, from the Home's Request that the last
	/// [`Self::replace_remote_authority`] read from its fresh description (no further network
	/// call) and the receiving Node's own Node Registration of the Home. `None` when the Home
	/// shares the receiver's Tenant, so the lists do not apply.
	async fn remote_tool_lists(&mut self) -> Result<Option<OwnedRemoteToolLists>>;
	/// The receiving Node's Federation Tool Block for the Home of a remote Run, read from its
	/// own Node Registration alone. `None` when it registers no such Node.
	async fn remote_tool_block(&mut self) -> Result<Option<ToolList>>;
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
