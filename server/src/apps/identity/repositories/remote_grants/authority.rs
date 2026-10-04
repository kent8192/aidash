//! Source authorization delegates through the same borrowed Access transaction.
use crate::authorization::access::Access;
use aidash_application::{Result, ports::authorization::source::SourceAuthorityScope};
use aidash_domain::{
	Task,
	policy::{PolicyBundle, Resource},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct Scope<'a> {
	pub(crate) access: &'a mut Access,
}
#[async_trait]
impl SourceAuthorityScope for Scope<'_> {
	fn source_subjects(&self) -> &[String] {
		&self.access.subjects
	}
	fn source_bundle(&self) -> &PolicyBundle {
		&self.access.snapshot.bundle
	}
	fn source_context(&mut self, attributes: Value) {
		self.access.context = attributes;
	}
	fn source_resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn generation_home(
		&mut self,
		task: &Task,
		node: &str,
		generation: Option<&Value>,
	) -> Result<()> {
		crate::generation::foreign::check_home(self.access, task, node, generation)
			.await
			.map_err(Into::into)
	}
	async fn source_workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	async fn source_task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.access.task_resource(task).await.map_err(Into::into)
	}
	async fn source_require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
}

#[async_trait]
impl aidash_application::ports::authorization::source::SourcePeerScope for Scope<'_> {
	async fn peer(&mut self, node: &str) -> Result<Option<aidash_domain::federation::Peer>> {
		super::persistence::peer(self.access, node)
			.await
			.map_err(Into::into)
	}
}
