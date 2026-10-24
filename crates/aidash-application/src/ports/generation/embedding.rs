//! Embedding authority borrows the existing lease; accounting owns one transaction.
use crate::{
	Result,
	ports::{catalog::CatalogScope, generation::publication::GenerationLive},
};
use aidash_domain::generation::{embedding::Attempt, policy::Spec, requests::Request};
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait GenerationEmbeddingAuthority: Send {
	/// Return generated ancestors from current subjects, sorted by request ID.
	async fn requests(&mut self, node: &str) -> Result<Vec<Request>>;
	async fn pinned_policy(&mut self, job: &Request) -> Result<Spec>;
	fn live(&mut self) -> &mut dyn GenerationLive;
	fn catalog(&mut self) -> &mut dyn CatalogScope;
}

/// Dropping a session rolls back every provisional ancestor charge or report.
#[async_trait]
pub trait GenerationEmbeddingSession: Send {
	async fn charge(&mut self, request: Uuid, amount: i64) -> Result<()>;
	async fn reserve(&mut self, request: Uuid, attempt: &Attempt) -> Result<()>;
	async fn refund(&mut self, request: Uuid, amount: i64) -> Result<()>;
	async fn report(&mut self, request: Uuid, attempt: Uuid, reported: Option<i64>) -> Result<()>;
	async fn commit(self: Box<Self>) -> Result<()>;
}

#[async_trait]
pub trait GenerationEmbeddingRepository: Send + Sync {
	fn node_id(&self) -> &str;
	async fn begin(&self) -> Result<Box<dyn GenerationEmbeddingSession>>;
}
