//! Summary Stage call reservations reuse the inherited generated-ancestor authority.
use crate::Result;
use aidash_domain::generation::summary::Attempt;
use async_trait::async_trait;
use uuid::Uuid;

/// Drop rolls back every provisional call charge and usage record.
#[async_trait]
pub trait GenerationSummarySession: Send {
	/// Charge one Summary Stage call; false when the ancestor's allowance is exhausted.
	async fn charge(&mut self, request: Uuid) -> Result<bool>;
	async fn reserve(&mut self, request: Uuid, attempt: &Attempt) -> Result<()>;
	async fn commit(self: Box<Self>) -> Result<()>;
}

#[async_trait]
pub trait GenerationSummaryRepository: Send + Sync {
	fn node_id(&self) -> &str;
	async fn begin(&self) -> Result<Box<dyn GenerationSummarySession>>;
}
