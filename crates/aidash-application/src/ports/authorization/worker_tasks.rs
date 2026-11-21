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
	/// None for an ordinary Thread; otherwise the active Subnode members.
	/// The Subnode membership lock is held through the delegated effect.
	async fn subnode_members(&mut self, thread: Uuid) -> Result<Option<Vec<EntityRef>>>;
	async fn task_thread(&mut self, task: Uuid) -> Result<Option<Uuid>>;
	async fn assign(
		&mut self,
		task: Uuid,
		policy: &str,
		reason: &str,
		members: Option<&[EntityRef]>,
		draft: Option<&aidash_domain::generation::policy::Draft>,
	) -> Result<Assignment>;
	async fn record_output(&mut self, source: &RunMetadata, kind: &str, id: Uuid) -> Result<()>;
	async fn thread(&mut self, id: Uuid) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn related_tasks(&mut self, thread: Uuid, input: &NewTask) -> Result<()>;
	async fn create_task(
		&mut self,
		thread: Uuid,
		input: &NewTask,
		creator: &str,
		key: &str,
	) -> Result<Task>;
	async fn task_resource(&mut self, task: &Task) -> Result<Resource>;
	async fn insert_origin(&mut self, task: Uuid, source: Uuid) -> Result<()>;
	async fn created_origin(&mut self, task: Uuid) -> Result<CreatedTaskOrigin>;
	async fn task_read(&mut self, task: Uuid) -> Result<Task>;
	/// The Run that created `task` with `task_create`, if any.
	async fn task_source_run(&mut self, task: Uuid) -> Result<Option<Uuid>>;
	/// [`Self::task_read`] for the calling Run's own Task, saying which check
	/// refused. A Node that did not answer stays `Error::NodeUnreachable`.
	async fn own_task_read(
		&mut self,
		task: Uuid,
	) -> Result<std::result::Result<Task, crate::authorization::worker_tasks::OwnTaskRefusal>>;
	async fn execution_grant(&mut self, task: Uuid) -> Result<Option<ExecutionGrant>>;
	async fn delegate(&mut self, task: Uuid, agent: &EntityRef) -> Result<Delegation>;
}
