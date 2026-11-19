//! Thread mutations borrow the caller's original transaction, authority and audit scope.
use crate::Result;
use aidash_domain::{NewTask, Task, Thread, nodes::NodeName, policy::Resource};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait ThreadMutations: Send {
	fn identity(&self) -> (&str, &str);
	fn resource(&self, kind: &str, id: Uuid, attributes: Value) -> Resource;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn require_thread(&mut self, id: Uuid, action: &str) -> Result<()>;
	/// The Node a new Thread goes to: `requested`, or the Tenant's default Node.
	/// The subject needs `node.use` on it.
	async fn use_node(&mut self, requested: Option<&NodeName>) -> Result<NodeName>;
	async fn insert_thread(&mut self, id: Uuid, node: &NodeName, title: &str) -> Result<Thread>;
	async fn record_owner(&mut self, id: Uuid) -> Result<()>;
	async fn update_title(&mut self, id: Uuid, revision: i64, title: &str) -> Result<Thread>;
	async fn related_tasks(&mut self, thread: Uuid, input: &NewTask) -> Result<()>;
	async fn insert_task(
		&mut self,
		thread: Uuid,
		input: &NewTask,
		created_by: &str,
		key: Option<&str>,
	) -> Result<Task>;
	async fn task_resource(&mut self, task: &Task) -> Result<Resource>;
	async fn insert_message(
		&mut self,
		thread: Uuid,
		sender: &str,
		content: &str,
		key: &str,
	) -> Result<()>;
}
