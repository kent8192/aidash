//! A session scope shares current authority and the same locked persistence transaction.
use crate::Result;
use aidash_domain::{
	Task,
	capabilities::sessions::{Area, SessionRun},
	policy::Resource,
	registry::{AgentConfig, EntityRef},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait SessionScope: Send {
	fn tenant(&self) -> &str;
	fn principal(&self) -> &str;
	fn subjects(&self) -> &[String];
	fn admission(&self) -> Result<bool>;
	fn set_context(&mut self, context: Value);
	fn resource(&self, kind: &str, id: Uuid, attributes: Value) -> Resource;
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn message(&mut self, workspace: Uuid, id: Uuid) -> Result<()>;
	async fn agent(&mut self, reference: &EntityRef) -> Result<()>;
	async fn reference(&mut self, id: Uuid) -> Result<()>;
	async fn bind_task(&mut self, task: Uuid, thread: Uuid) -> Result<()>;
	async fn load(&mut self, id: Uuid) -> Result<Option<Area>>;
	async fn run_binding(&mut self, run_id: Uuid) -> Result<Option<(Uuid, i64)>>;
	async fn current_run(&mut self, area: &Area) -> Result<Option<Uuid>>;
	async fn context_area(&mut self, run_id: Uuid) -> Result<Option<Area>>;
	async fn explicit_thread(&mut self, task_id: Uuid) -> Result<Option<Uuid>>;
	async fn task(&mut self, id: Uuid) -> Result<Task>;
	async fn parent_areas(&mut self, id: Uuid) -> Result<Vec<Area>>;
	async fn lock_thread(&mut self, thread: Uuid, workspace: Uuid) -> Result<()>;
	async fn ensure_area(&mut self, task: &Task, thread: Uuid, agent_id: &str) -> Result<Area>;
	async fn initialized(&mut self, run_id: Uuid) -> Result<Option<bool>>;
	async fn initialized_locked(&mut self, run_id: Uuid) -> Result<bool>;
	async fn mark_initialized(&mut self, run_id: Uuid) -> Result<()>;
	async fn pin(&mut self, run_id: Uuid, area: &mut Area, config: &AgentConfig) -> Result<()>;
	async fn enqueue(&mut self, run_id: Uuid, area: &Area, initialized: bool) -> Result<()>;
	async fn queue(&mut self, area: &Area) -> Result<Vec<SessionRun>>;
	async fn last(&mut self, area: &Area) -> Result<Option<(Uuid, String)>>;
	async fn previous(&mut self, key: Uuid) -> Result<Option<(String, Value)>>;
	async fn cache(&mut self, key: Uuid, digest: &str, result: &Value) -> Result<()>;
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()>;
}
