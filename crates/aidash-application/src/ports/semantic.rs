//! A restored indexing scope owns its transaction and rolls back when cancelled.
use crate::Result;
use aidash_domain::semantic::{
	EmbeddingConfig, Source, VectorConfig,
	indexing::{IndexingEntry, IndexingPlan},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

pub struct CleanupBatch {
	pub points: Vec<Uuid>,
	pub collections: Vec<String>,
}

#[async_trait]
pub trait SemanticVisibility: Send {}

#[async_trait]
pub trait SemanticIndexingSession: Send {
	fn durable(&mut self);
	async fn workspace_write(&mut self, workspace: Uuid) -> Result<()>;
	async fn plan(&mut self, workspace: Uuid) -> Result<IndexingPlan>;
	/// Keep the original update/skip-locked selection and database-time due fence.
	async fn lock_entry(&mut self, id: Uuid) -> Result<Option<IndexingEntry>>;
	async fn authority(&mut self, id: Uuid) -> Result<Value>;
	/// Decide the requested action for the entry held by this scope.
	async fn permits(&mut self, action: &str) -> Result<bool>;
	async fn source(&mut self, workspace: Uuid, source: &Source) -> Result<Option<String>>;
	async fn set_revoked(&mut self, id: Uuid, reason: &str) -> Result<()>;
	async fn retire_points(&mut self, id: Uuid) -> Result<()>;
	async fn history(
		&mut self,
		workspace: Uuid,
		id: Uuid,
		revision: i64,
		state: &str,
		detail: &str,
	) -> Result<()>;
	async fn point(&mut self, id: Uuid) -> Result<Option<(Option<String>, bool)>>;
	async fn defer(&mut self, id: Uuid) -> Result<()>;
	async fn rotate(&mut self, id: Uuid, point: Uuid) -> Result<IndexingEntry>;
	async fn schedule_point(&mut self, collection: &str) -> Result<()>;
	/// Retain the same restored authority through generation allowance accounting.
	async fn embed(
		&mut self,
		workspace: Uuid,
		config: &EmbeddingConfig,
		text: &str,
		entry: Uuid,
	) -> Result<Vec<f32>>;
	async fn record_digest(&mut self, point: Uuid, digest: &str) -> Result<()>;
	async fn ready(&mut self, id: Uuid) -> Result<()>;
	async fn failed(&mut self, id: Uuid, attempts: i32, delay: f64) -> Result<()>;
	async fn finish(self: Box<Self>, result: Result<bool>) -> Result<bool>;
}

#[async_trait]
pub trait SemanticCleanupSession: Send {
	async fn lock_point(&mut self, id: Uuid) -> Result<Option<(String, VectorConfig)>>;
	async fn lock_collection(&mut self, collection: &str) -> Result<Option<VectorConfig>>;
	async fn record_point(&mut self, id: Uuid, failure: Option<&str>) -> Result<()>;
	async fn record_collection(&mut self, collection: &str, failure: Option<&str>) -> Result<()>;
	async fn commit(self: Box<Self>) -> Result<()>;
}

#[async_trait]
pub trait SemanticIndexingRepository: Send + Sync {
	async fn begin_visibility(&self) -> Result<Box<dyn SemanticVisibility>>;
	/// Return at most 32 entries ordered by the original due time and ID.
	async fn due(&self) -> Result<Vec<Uuid>>;
	async fn initial(&self, id: Uuid) -> Result<Option<(Uuid, Value)>>;
	async fn restore(&self, authority: &Value) -> Result<Box<dyn SemanticIndexingSession>>;
	async fn revoke(&self, workspace: Uuid, id: Uuid, authority: &Value) -> Result<()>;
	async fn cleanup_due(&self) -> Result<CleanupBatch>;
	async fn begin_cleanup(&self) -> Result<Box<dyn SemanticCleanupSession>>;
}

pub mod mutations;

pub mod retrieval;

pub mod embedding;

pub mod memory;

pub mod visibility;

pub mod run_context;

pub mod remote_journal;

pub mod remote_status;
