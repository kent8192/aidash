//! Worker-created tasks and assignments borrow the inherited authority transaction.
use crate::Result;
use aidash_domain::{
	NewTask, RunMetadata, Task,
	federation::Delegation,
	generation::requests::Assignment,
	identity::execution::{CreatedTaskOrigin, ExecutionGrant, ExecutionPrincipal},
	policy::Resource,
	registry::EntityRef,
};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait WorkerTaskScope: Send {
	fn node_id(&self) -> &str;
	fn identity(&self) -> ExecutionPrincipal;
	fn subjects(&self) -> &[String];
	async fn task_workspace(&mut self, task: Uuid) -> Result<Option<Uuid>>;
	async fn assign(&mut self, task: Uuid, policy: &str, reason: &str) -> Result<Assignment>;
	async fn record_output(&mut self, source: &RunMetadata, kind: &str, id: Uuid) -> Result<()>;
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn related_tasks(&mut self, workspace: Uuid, input: &NewTask) -> Result<()>;
	async fn create_task(
		&mut self,
		workspace: Uuid,
		input: &NewTask,
		creator: &str,
		key: &str,
	) -> Result<Task>;
	async fn task_resource(&mut self, task: &Task) -> Result<Resource>;
	async fn insert_origin(&mut self, task: Uuid, source: Uuid) -> Result<()>;
	async fn created_origin(&mut self, task: Uuid) -> Result<CreatedTaskOrigin>;
	async fn task_read(&mut self, task: Uuid) -> Result<Task>;
	async fn execution_grant(&mut self, task: Uuid) -> Result<Option<ExecutionGrant>>;
	async fn delegate(&mut self, task: Uuid, agent: &EntityRef) -> Result<Delegation>;
}
