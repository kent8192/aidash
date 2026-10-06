//! Native semantic journals share source policy while keeping their ORM absence and lock contracts.
use super::NativeReads;
use aidash_application::{
	Result,
	ports::semantic::{memory::SemanticMemoryReadSession, visibility::SemanticDisclosureScope},
};
use aidash_domain::{
	Artifact, Message,
	policy::Resource,
	semantic::{Source, mutations::Entry},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
impl SemanticDisclosureScope for NativeReads<'_> {
	async fn unit(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<String>> {
		let mut lease = crate::semantic::service::Lease::Inherited(self.access);
		match crate::apps::knowledge::repositories::units::text(&mut lease, id, workspace).await {
			Err(crate::Error::Forbidden | crate::Error::Conflict(_)) => Ok(None),
			result => result.map_err(Into::into),
		}
	}

	fn scoped(&self) -> bool {
		true
	}
	// NativeReads always retains subject authority; there is no operator branch.
	async fn operator_visible(&mut self, _: Uuid) -> Result<bool> {
		Ok(true)
	}
	async fn workspace_resource(&mut self, id: Uuid) -> Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	fn resource(&mut self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn artifact(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Artifact>> {
		crate::apps::workspaces::models::Artifact::read_in(
			self.access.tx.as_mut(),
			id,
			workspace,
			true,
		)
		.await
		.map_err(Into::into)
	}
	async fn message(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Message>> {
		crate::apps::workspaces::models::Message::read_in(
			self.access.tx.as_mut(),
			id,
			workspace,
			true,
		)
		.await
		.map_err(Into::into)
	}
	async fn artifact_visible(&mut self, row: &Artifact) -> Result<bool> {
		Box::pin(self.access.artifact_visible(row))
			.await
			.map_err(Into::into)
	}
	async fn message_visible(&mut self, row: &Message) -> Result<bool> {
		Box::pin(self.access.message_visible(row))
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl SemanticMemoryReadSession for NativeReads<'_> {
	async fn native_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		crate::apps::knowledge::repositories::memory_reads::visible(
			&mut crate::semantic::service::Lease::Inherited(self.access),
			run,
		)
		.await
		.map_err(Into::into)
	}
	async fn permits(&mut self, entry: &Entry, action: &str) -> Result<bool> {
		aidash_application::semantic::visibility::permits(self, entry, action).await
	}
	async fn source(&mut self, workspace: Uuid, source: &Source) -> Result<Option<String>> {
		aidash_application::semantic::visibility::source(self, workspace, source).await
	}
	async fn dependencies(&mut self, run: Uuid) -> Result<Vec<(Uuid, i64)>> {
		crate::apps::knowledge::models::SemanticRunRead::for_run(self.access.tx.as_mut(), run)
			.await
			.map_err(Into::into)
	}
	async fn entry(&mut self, id: Uuid) -> Result<Option<Entry>> {
		crate::apps::knowledge::models::SemanticEntry::read_in(self.access.tx.as_mut(), id)
			.await
			.map(|entry| entry.map(Into::into))
			.map_err(Into::into)
	}
	async fn point_digest(&mut self, id: Uuid) -> Result<Option<String>> {
		crate::apps::knowledge::models::SemanticPoint::digest_in(self.access.tx.as_mut(), id)
			.await
			.map_err(Into::into)
	}
}
