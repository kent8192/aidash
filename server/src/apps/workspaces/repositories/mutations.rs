//! Native workspace writes keep atomic storage operations in the existing Access transaction.
use crate::{apps::identity::services::access::Access, store::Store};
use aidash_application::{Result, ports::workspaces::mutations::WorkspaceMutations};
use aidash_domain::{NewTask, Task, Workspace, nodes::NodeName, policy::Resource};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct NativeMutations<'a> {
	pub(crate) access: &'a mut Access,
	pub(crate) store: &'a Store,
}
#[async_trait]
impl WorkspaceMutations for NativeMutations<'_> {
	fn identity(&self) -> (&str, &str) {
		(&self.access.identity.tenant, &self.access.identity.subject)
	}
	fn resource(&self, kind: &str, id: Uuid, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn require_workspace(&mut self, id: Uuid, action: &str) -> Result<()> {
		self.access
			.require_workspace(id, action)
			.await
			.map_err(Into::into)
	}
	async fn use_node(&mut self, requested: Option<&NodeName>) -> Result<NodeName> {
		let tenant = self.access.identity.tenant.clone();
		let node = match requested {
			Some(name) => {
				crate::apps::identity::repositories::nodes::get(
					&mut **self.access.tx,
					&tenant,
					name.as_str(),
				)
				.await?
			}
			None => {
				crate::apps::identity::repositories::nodes::default_of(
					&mut **self.access.tx,
					&tenant,
				)
				.await?
			}
		};
		let resource = self.access.resource(
			"node",
			node.name.as_str(),
			serde_json::json!({"label": node.label, "default": node.default}),
		);
		self.access.require(&resource, "node.use").await?;
		self.access.set_node(node.name.clone());
		Ok(node.name)
	}
	async fn insert_workspace(
		&mut self,
		id: Uuid,
		node: &NodeName,
		title: &str,
		goal: &str,
	) -> Result<Workspace> {
		self.store
			.create_workspace_in(
				&mut self.access.tx,
				id,
				&self.access.identity.tenant,
				node,
				title,
				goal,
			)
			.await
			.map_err(Into::into)
	}
	async fn update_state(&mut self, id: Uuid, revision: i64, state: Value) -> Result<Workspace> {
		self.store
			.update_state_in(&mut self.access.tx, id, revision, state)
			.await
			.map_err(Into::into)
	}
	async fn related_tasks(&mut self, workspace: Uuid, input: &NewTask) -> Result<()> {
		self.access
			.related_tasks(workspace, input)
			.await
			.map_err(Into::into)
	}
	async fn insert_task(
		&mut self,
		workspace: Uuid,
		input: &NewTask,
		created_by: &str,
		key: Option<&str>,
	) -> Result<Task> {
		self.store
			.create_task_in(&mut self.access.tx, workspace, input, created_by, key)
			.await
			.map_err(Into::into)
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.access.task_resource(task).await.map_err(Into::into)
	}
	async fn insert_message(
		&mut self,
		workspace: Uuid,
		sender: &str,
		content: &str,
		key: &str,
	) -> Result<()> {
		self.store
			.message_in(&mut self.access.tx, workspace, sender, content, Some(key))
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn record_owner(&mut self, id: Uuid) -> Result<()> {
		self.store
			.record_workspace_tenant_in(
				&mut self.access.tx,
				id,
				&self.access.identity.tenant,
				&self.access.identity.subject,
			)
			.await
			.map_err(Into::into)
	}
}
