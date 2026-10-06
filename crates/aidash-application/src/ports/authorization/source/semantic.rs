//! Explicit semantic disclosure uses current source authority and index configuration.
use crate::{Result, ports::authorization::source::SourceAuthorityScope};
use aidash_domain::{
	generation::remote::Ancestor,
	registry::{EntityRef, Entry},
	semantic::{indexing::IndexingSpec, mutations::Index},
};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait SemanticBindingScope: SourceAuthorityScope {
	fn home_node_id(&self) -> &str;
	fn binding_tenant(&self) -> &str;
	/// Decode native defaults before crossing the boundary, retaining the exact stored specification digest.
	async fn semantic_index(&mut self, workspace: Uuid) -> Result<(Index, IndexingSpec)>;
	async fn embedding_entry(&mut self, entry: &EntityRef, action: &str) -> Result<Entry>;
	async fn binding_lineage(&mut self) -> Result<Vec<Ancestor>>;
}
