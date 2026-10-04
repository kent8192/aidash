//! Source authority borrows the caller's current policy, credential and grant transaction.
use crate::Result;
use aidash_domain::{
	Task,
	policy::{PolicyBundle, Resource},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait SourceAuthorityScope: Send {
	fn source_subjects(&self) -> &[String];
	fn source_bundle(&self) -> &PolicyBundle;
	fn source_context(&mut self, attributes: Value);
	fn source_resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn generation_home(
		&mut self,
		task: &Task,
		node: &str,
		generation: Option<&Value>,
	) -> Result<()>;
	async fn source_workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn source_task_resource(&mut self, task: &Task) -> Result<Resource>;
	async fn source_require(&mut self, resource: &Resource, action: &str) -> Result<()>;
}

pub mod grants;

#[async_trait]
pub trait SourcePeerScope: Send {
	async fn peer(&mut self, node: &str) -> Result<Option<aidash_domain::federation::Peer>>;
}

pub mod semantic;

pub mod reads;

pub mod provenance;
