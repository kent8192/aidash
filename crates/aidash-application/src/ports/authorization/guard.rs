//! Guard checks borrow the worker's retained credential, policy, and source leases.
use crate::Result;
use aidash_domain::{
	RunMetadata, Task,
	policy::{PolicyBundle, Resource},
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait RunGuardScope: Send {
	fn node_id(&self) -> &str;
	fn bundle(&self) -> &PolicyBundle;
	async fn run_visible(&mut self, run: &RunMetadata) -> Result<bool>;
	async fn cluster_targets(&mut self, workspace: Uuid) -> Result<Vec<String>>;
	async fn catalog(&mut self, reference: &EntityRef, action: &str) -> Result<Entry>;
	async fn task_read(&mut self, task: Uuid) -> Result<Task>;
	async fn task_resource(&mut self, task: &Task) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn require_live(&mut self, task: Uuid, agent: &EntityRef) -> Result<()>;
	async fn check_pinned(&mut self, entry: &Entry) -> Result<()>;
	async fn context_authority(&mut self, run: &RunMetadata) -> Result<()>;
}
