//! Summary Stage call reservations reuse the inherited generated-ancestor authority.
use crate::{
	Result,
	ports::{catalog::CatalogScope, generation::publication::GenerationLive},
};
use aidash_domain::generation::summary::Attempt;
use async_trait::async_trait;
use serde_json::Value;
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

/// Current generated ancestors and their live authority.
#[async_trait]
pub trait GenerationSummaryAuthority: Send {
	async fn ancestors(&mut self, node: &str) -> Result<Vec<(Uuid, Value)>>;
	fn live(&mut self) -> &mut dyn GenerationLive;
	fn catalog(&mut self) -> &mut dyn CatalogScope;
}

pub mod remote;
