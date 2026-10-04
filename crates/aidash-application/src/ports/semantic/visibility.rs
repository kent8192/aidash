//! Source disclosure borrows the same authority and row locks as its caller.
use crate::Result;
use aidash_domain::{Artifact, Message, policy::Resource};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait SemanticDisclosureScope: Send {
	fn scoped(&self) -> bool;
	async fn operator_visible(&mut self, workspace: Uuid) -> Result<bool>;
	async fn workspace_resource(&mut self, workspace: Uuid) -> Result<Resource>;
	fn resource(&mut self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	async fn managed_memory(&mut self, id: Uuid) -> Result<Option<(String, String)>>;
	/// Both source reads retain the original shared lock, including operator reads.
	async fn artifact(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Artifact>>;
	async fn message(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Message>>;
	async fn artifact_visible(&mut self, artifact: &Artifact) -> Result<bool>;
	async fn message_visible(&mut self, message: &Message) -> Result<bool>;
}
