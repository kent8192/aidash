//! Native semantic binding ports retain the original index lease, defaults and catalog authority.
use crate::{authorization::access::Access, federation::Federation};
use aidash_application::{
	Result,
	ports::authorization::source::{SourceAuthorityScope, semantic::SemanticBindingScope},
};
use aidash_domain::{
	Task,
	generation::remote::Ancestor,
	policy::{PolicyBundle, Resource},
	registry::{EntityRef, Entry},
	semantic::{indexing::IndexingSpec, mutations::Index},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct Scope<'a> {
	pub(crate) runtime: &'a Federation,
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
		crate::bootstrap::source_authority_scope(self.access)
			.generation_home(task, node, generation)
			.await
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
impl SemanticBindingScope for Scope<'_> {
	fn home_node_id(&self) -> &str {
		&self.runtime.config.node_id
	}
	fn binding_tenant(&self) -> &str {
		&self.access.identity.tenant
	}
	async fn semantic_index(&mut self, workspace: Uuid) -> Result<(Index, IndexingSpec)> {
		let index = crate::semantic::service::index(&mut self.access.tx, workspace, false).await?;
		let spec = index.configuration()?;
		Ok((index.into(), spec.into()))
	}
	async fn embedding_entry(&mut self, entry: &EntityRef, action: &str) -> Result<Entry> {
		crate::authorization::catalog::entry(self.access, entry, action)
			.await
			.map_err(Into::into)
	}
	async fn binding_lineage(&mut self) -> Result<Vec<Ancestor>> {
		crate::generation::remote::lineage(self.access, &self.runtime.config.node_id)
			.await
			.map_err(Into::into)
	}
}
