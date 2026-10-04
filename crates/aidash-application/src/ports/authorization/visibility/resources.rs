//! Resource policies and event readers share the caller's complete provenance lease.
use crate::Result;
use aidash_domain::{Artifact, HumanRequest, Message, Task, policy::Resource};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait ResourceVisibilityScope: Send {
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	async fn artifact_task(&mut self, artifact: &Artifact) -> Result<Option<Task>>;
	async fn output_visible(&mut self, workspace: Uuid, kind: &str, id: Uuid) -> Result<bool>;
	fn cached_human(&self, id: Uuid) -> Option<bool>;
	fn remember_human(&mut self, id: Uuid, allowed: bool);
	async fn humans(&mut self, workspace: Uuid, run: Uuid) -> Result<Vec<HumanRequest>>;
}
#[async_trait]
pub trait ResourceAccessScope: ResourceVisibilityScope {
	async fn task_any(&mut self, id: Uuid) -> Result<Option<Task>>;
}
#[async_trait]
pub trait ResourceEventScope: ResourceVisibilityScope {
	async fn event_task(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<Option<Task>>;
	async fn artifact(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<Option<Artifact>>;
	async fn messages_id(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<Vec<Message>>;
	async fn messages_legacy(
		&mut self,
		workspace: Option<Uuid>,
		sender: Option<&str>,
		content: Option<&str>,
	) -> Result<Vec<Message>>;
}
