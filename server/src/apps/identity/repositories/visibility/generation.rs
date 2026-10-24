//! Native generation disclosure borrows the same ORM transaction and policy state.
use super::NativeReads;
use crate::apps::identity::services::access::NativeAccess;
use aidash_application::{Result, ports::generation::visibility::GenerationVisibility};
use aidash_domain::{Task, policy::Resource};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct NativeGeneration<'a> {
	pub access: &'a mut NativeAccess,
}
#[async_trait]
impl GenerationVisibility for NativeGeneration<'_> {
	fn inherited_lease(&self) -> bool {
		self.access.inherited_lease()
	}
	fn context(&mut self, value: Value) {
		self.access.set_context(value);
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	async fn task(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Task>> {
		crate::apps::workspaces::models::Task::read_in(self.access.tx.as_mut(), id, workspace)
			.await
			.map_err(Into::into)
	}
	async fn task_visible(&mut self, task: &Task) -> Result<bool> {
		aidash_application::authorization::visibility::resources::task_visible(
			&mut NativeReads {
				access: self.access,
			},
			task,
		)
		.await
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
}
