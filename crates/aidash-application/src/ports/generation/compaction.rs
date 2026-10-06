//! Compaction keeps inherited authority and atomic call reservations behind ports.
use crate::{
	Result,
	ports::{
		CompactionClassifier, CompactionQuestions, catalog::CatalogScope,
		generation::publication::GenerationLive,
	},
};
use aidash_domain::{generation::compaction::Attempt, registry::CompactorConfig};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

#[async_trait]
pub trait GenerationCompactionAuthority: Send {
	/// Return request IDs and pinned policy documents in stable request-ID order.
	async fn ancestors(&mut self, node: &str) -> Result<Vec<(Uuid, Value)>>;
	fn live(&mut self) -> &mut dyn GenerationLive;
	fn catalog(&mut self) -> &mut dyn CatalogScope;
}

pub trait ApprovedCompactionTransport: CompactionClassifier {
	/// Validate the exact request against the approved provider's bounds.
	fn check_request(&self, state: &Value, questions: &CompactionQuestions) -> Result<usize>;
}

pub trait GenerationCompactionProvider: Send + Sync {
	fn approved(&self, config: CompactorConfig) -> Result<Arc<dyn ApprovedCompactionTransport>>;
}

/// Drop rolls back every provisional call charge and usage record.
#[async_trait]
pub trait GenerationCompactionSession: Send {
	async fn charge(&mut self, request: Uuid) -> Result<bool>;
	async fn reserve(&mut self, request: Uuid, attempt: &Attempt) -> Result<()>;
	async fn commit(self: Box<Self>) -> Result<()>;
}

#[async_trait]
pub trait GenerationCompactionRepository: Send + Sync {
	fn node_id(&self) -> &str;
	async fn begin(&self) -> Result<Box<dyn GenerationCompactionSession>>;
}

pub mod remote;
