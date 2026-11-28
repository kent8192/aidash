//! A session scope shares current authority and the same locked persistence transaction.
use crate::Result;
use aidash_domain::{
	RunMetadata, Task,
	capabilities::sessions::Area,
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
	async fn thread(&mut self, id: Uuid) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn message(&mut self, thread: Uuid, id: Uuid) -> Result<()>;
	async fn agent(&mut self, reference: &EntityRef) -> Result<()>;
	/// Whether Reference files an Area was given can still be used (ADR 0114).
	async fn references(&mut self, ids: &[Uuid]) -> Result<()>;
	async fn load(&mut self, id: Uuid) -> Result<Option<Area>>;
	async fn run_binding(&mut self, run_id: Uuid) -> Result<Option<Uuid>>;
	async fn context_area(&mut self, run_id: Uuid) -> Result<Option<Area>>;
	async fn task(&mut self, id: Uuid) -> Result<Task>;
	async fn parent_areas(&mut self, id: Uuid) -> Result<Vec<Area>>;
	/// Lock the Thread whose Working Area a Run is admitted to, refusing a
	/// deleted one.
	async fn lock_thread(&mut self, thread: Uuid) -> Result<()>;
	/// The Working Area of (home Node, Thread, Agent, owner) for `task`.
	async fn ensure_area(&mut self, task: &Task, agent_id: &str) -> Result<Area>;
	/// The Area of a Run executing here for another Home, created empty when absent and kept
	/// with its `remote_home` constraint. It is keyed by this Tenant, the Run's Home Node and
	/// Home Thread, the Run's Agent and the Home person `owner` (ADR 0139, #1008).
	async fn ensure_remote_area(&mut self, run: &RunMetadata, owner: &str) -> Result<Area>;
	async fn initialized(&mut self, run_id: Uuid) -> Result<Option<bool>>;
	async fn initialized_locked(&mut self, run_id: Uuid) -> Result<bool>;
	async fn mark_initialized(&mut self, run_id: Uuid) -> Result<()>;
	async fn pin(&mut self, run_id: Uuid, area: &mut Area, config: &AgentConfig) -> Result<()>;
	async fn bind(&mut self, run_id: Uuid, area: &Area, initialized: bool) -> Result<()>;
	async fn active_run(&mut self, area: &Area) -> Result<Option<Uuid>>;
	async fn last(&mut self, area: &Area) -> Result<Option<(Uuid, String)>>;
	async fn previous(&mut self, key: Uuid) -> Result<Option<(String, Value)>>;
	async fn cache(&mut self, key: Uuid, digest: &str, result: &Value) -> Result<()>;
	async fn event(&mut self, thread: Uuid, kind: &str, data: Value) -> Result<()>;
}
