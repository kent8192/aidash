//! Inference approval borrows the retained worker Access scope and saved Run resources.
use crate::Result;
use aidash_domain::{RunMetadata, policy::Resource, registry::EntityRef};
use async_trait::async_trait;
#[async_trait]
pub trait InferenceScope: Send {
	fn remote(&self) -> bool;
	fn node_id(&self) -> Option<&str>;
	async fn context_authority(&mut self, run: &RunMetadata) -> Result<()>;
	async fn require_live(
		&mut self,
		node: &str,
		run: &RunMetadata,
		agent: &EntityRef,
	) -> Result<()>;
	async fn catalog(&mut self, reference: &EntityRef, action: &str) -> Result<()>;
	async fn memory_resource(&mut self, run: &RunMetadata) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
}
