//! Current definitions and private context are read before resolving prompt contracts.
use crate::{Result, registry::DefinitionValidation};
use aidash_domain::{RunMetadata, registry::Entry};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait Definitions: Send + Sync {
	fn node(&self) -> &str;
	async fn definition(&self, run: &RunMetadata, id: &str, version: &str) -> Result<Entry>;
	async fn documents(&self, agent: &Entry) -> Result<Value>;
	/// Resolve the current capability profile after definition and knowledge reads.
	fn validation(&self) -> DefinitionValidation;
	async fn pinned_headroom(&self, run: Uuid) -> Result<usize>;
}
