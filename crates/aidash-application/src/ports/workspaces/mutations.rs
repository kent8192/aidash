//! Workspace mutations borrow the caller's original transaction, authority and audit scope.
use crate::Result;
use aidash_domain::{NewTask, Task, Workspace, policy::Resource};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait WorkspaceMutations: Send {
	fn identity(&self) -> (&str, &str);
	fn resource(&self, kind: &str, id: Uuid, attributes: Value) -> Resource;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn require_workspace(&mut self, id: Uuid, action: &str) -> Result<()>;
	async fn insert_workspace(&mut self, id: Uuid, title: &str, goal: &str) -> Result<Workspace>;
	async fn record_owner(&mut self, id: Uuid) -> Result<()>;
	async fn update_state(&mut self, id: Uuid, revision: i64, state: Value) -> Result<Workspace>;
	async fn related_tasks(&mut self, workspace: Uuid, input: &NewTask) -> Result<()>;
	async fn insert_task(
		&mut self,
		workspace: Uuid,
		input: &NewTask,
		created_by: &str,
		key: Option<&str>,
	) -> Result<Task>;
	async fn task_resource(&mut self, task: &Task) -> Result<Resource>;
	async fn insert_message(
		&mut self,
		workspace: Uuid,
		sender: &str,
		content: &str,
		key: &str,
	) -> Result<()>;
}
