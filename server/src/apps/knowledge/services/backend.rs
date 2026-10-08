//! Adapt the injected embedding and vector ports to the existing service boundary.
use super::{EmbeddingConfig, VectorConfig};
use crate::Result;
use aidash_application::ports::{EmbeddingProvider, VectorIndex};
pub use aidash_domain::semantic::{Embedding, Point, VectorFilter as Filter};
use serde_json::Value;
use uuid::Uuid;

pub fn client() -> Result<reqwest::Client> {
	aidash_integrations::semantic::client().map_err(Into::into)
}
pub async fn embed(
	store: &crate::store::Store,
	config: &EmbeddingConfig,
	text: &str,
) -> Result<Embedding> {
	crate::bootstrap::semantic_transport(store)
		.embed(config, text)
		.await
		.map_err(Into::into)
}
pub async fn ensure_collection(
	store: &crate::store::Store,
	config: &VectorConfig,
	collection: &str,
	dimensions: usize,
) -> Result<()> {
	crate::bootstrap::semantic_transport(store)
		.ensure_collection(config, collection, dimensions)
		.await
		.map_err(Into::into)
}
pub async fn upsert(
	store: &crate::store::Store,
	config: &VectorConfig,
	collection: &str,
	point: Uuid,
	vector: &[f32],
	payload: Value,
) -> Result<()> {
	crate::bootstrap::semantic_transport(store)
		.upsert(config, collection, point, vector, payload)
		.await
		.map_err(Into::into)
}
pub async fn delete_point(
	store: &crate::store::Store,
	config: &VectorConfig,
	collection: &str,
	point: Uuid,
) -> Result<()> {
	crate::bootstrap::semantic_transport(store)
		.delete_point(config, collection, point)
		.await
		.map_err(Into::into)
}
pub async fn delete_collection(
	store: &crate::store::Store,
	config: &VectorConfig,
	collection: &str,
) -> Result<()> {
	crate::bootstrap::semantic_transport(store)
		.delete_collection(config, collection)
		.await
		.map_err(Into::into)
}
pub async fn query(
	store: &crate::store::Store,
	config: &VectorConfig,
	collection: &str,
	vector: &[f32],
	filter: Filter<'_>,
	limit: usize,
) -> Result<Vec<Point>> {
	crate::bootstrap::semantic_transport(store)
		.query(config, collection, vector, filter, limit)
		.await
		.map_err(Into::into)
}
pub async fn present(
	store: &crate::store::Store,
	config: &VectorConfig,
	collection: &str,
	ids: &[Uuid],
) -> Result<bool> {
	crate::bootstrap::semantic_transport(store)
		.present(config, collection, ids)
		.await
		.map_err(Into::into)
}
