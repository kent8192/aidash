//! Retrieved semantic text is disclosed only after its durable Run dependency journal commits.
use crate::Result;
use aidash_domain::{
	Task,
	semantic::{InputRead, results::SearchResult},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait RunSemanticJournal: Send {
	async fn record(&mut self, entry: Uuid, revision: i64) -> Result<()>;
	async fn commit(self: Box<Self>) -> Result<()>;
}
#[async_trait]
pub trait RunSemanticScope: Send {
	async fn retrieve(&mut self, query: &str, budget: usize) -> Result<Option<SearchResult>>;
	/// Authority and source locks remain held by this scope through journal commit.
	async fn begin_dependencies(&mut self) -> Result<Box<dyn RunSemanticJournal + '_>>;
}
#[async_trait]
pub trait RunSemanticRepository: Send + Sync {
	fn remote(&self) -> bool;
	async fn suspend(&self) -> Result<()>;
	async fn remote_context(
		&self,
		task: &Task,
		inputs: &[(InputRead, String)],
		query: &str,
		budget: usize,
	) -> Result<Option<Value>>;
	async fn refresh_remote(&self) -> Result<()>;
	async fn local_scope(&self) -> Result<Box<dyn RunSemanticScope + '_>>;
}
