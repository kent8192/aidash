//! Workspace ownership and interaction borrow current authority and existing row locks.
use crate::{Result, ports::authorization::lease::AuthorizationLease};
use aidash_domain::{Run, RunMetadata, policy::Resource};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait WorkspaceResourceScope: Send {
	fn inherited(&self) -> bool;
	fn context(&self) -> &Value;
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn owner(&mut self, id: Uuid, lock: bool) -> Result<Option<String>>;
}
#[async_trait]
pub trait WorkspaceAuthorityScope: WorkspaceResourceScope + AuthorizationLease {
	async fn locked_owner(&mut self, id: Uuid) -> Result<Option<String>>;
	fn identity(&self) -> (&str, &str);
	async fn all_owners(&mut self) -> Result<Vec<(Uuid, String)>>;
	async fn event_owners(&mut self, selected: Option<Uuid>) -> Result<Vec<(Uuid, String)>>;
}
#[async_trait]
pub trait RunInteractionScope: WorkspaceResourceScope {
	async fn run(&mut self, id: Uuid) -> Result<Option<Run>>;
	fn set_context(&mut self, context: Value);
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn run_visible(&mut self, run: &RunMetadata) -> Result<bool>;
}
