//! Mutation scopes retain their original index, credential, and policy locks.
use crate::Result;
use aidash_domain::semantic::{
	Source,
	indexing::IndexingSpec,
	mutations::{Entry, History, Index},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait SemanticConfigurationRepository: Send + Sync {
	/// Resolve credentials and validate the existing local provider contract before opening a scope.
	fn validate(&self, spec: &IndexingSpec) -> Result<()>;
	async fn begin(&self) -> Result<Box<dyn SemanticConfigurationSession>>;
}

#[async_trait]
pub trait SemanticConfigurationSession: Send {
	async fn lock_workspace(&mut self, workspace: Uuid) -> Result<()>;
	async fn tenant(&mut self, workspace: Uuid) -> Result<String>;
	async fn index(&mut self, workspace: Uuid) -> Result<Option<Index>>;
	async fn counts(&mut self, workspace: Uuid) -> Result<(i64, usize)>;
	async fn replace(
		&mut self,
		workspace: Uuid,
		tenant: &str,
		revision: i64,
		spec: Value,
		collection: &str,
	) -> Result<Index>;
	async fn retire_collections(&mut self, workspace: Uuid) -> Result<()>;
	async fn record_collection(
		&mut self,
		workspace: Uuid,
		collection: &str,
		vector: Value,
	) -> Result<()>;
	async fn active(&mut self, workspace: Uuid) -> Result<Vec<Uuid>>;
	async fn reconfigure(&mut self, id: Uuid, revision: i64) -> Result<Entry>;
	async fn schedule(&mut self, entry: &Entry, collection: &str) -> Result<()>;
	async fn history(&mut self, workspace: Uuid, revision: i64) -> Result<()>;
	async fn commit(self: Box<Self>) -> Result<()>;
}

#[async_trait]
pub trait SemanticEntriesSession: Send {
	async fn workspace(&mut self, workspace: Uuid, action: &str) -> Result<()>;
	async fn index(&mut self, workspace: Uuid, exclusive: bool) -> Result<Index>;
	async fn by_key(&mut self, workspace: Uuid, key: &str) -> Result<Option<Entry>>;
	fn saved(&mut self) -> Result<Value>;
	async fn permits(&mut self, entry: &Entry, action: &str) -> Result<bool>;
	async fn source(&mut self, workspace: Uuid, source: &Source) -> Result<Option<String>>;
	async fn count(&mut self, workspace: Uuid) -> Result<i64>;
	async fn put(&mut self, entry: Entry, authority: Value) -> Result<Entry>;
	async fn schedule(&mut self, entry: &Entry, collection: &str) -> Result<()>;
	async fn history(
		&mut self,
		workspace: Uuid,
		id: Uuid,
		revision: i64,
		state: &str,
		detail: &str,
	) -> Result<()>;
	async fn list(&mut self, workspace: Uuid) -> Result<Vec<Entry>>;
	async fn lock_entry(&mut self, workspace: Uuid, id: Uuid) -> Result<Option<Entry>>;
	async fn reindex_replay(&mut self, id: Uuid, revision: i64) -> Result<bool>;
	async fn managed_memory(&mut self, id: Uuid) -> Result<Option<(String, String, String)>>;
	async fn require_memory_write(
		&mut self,
		workspace: Uuid,
		agent: &str,
		version: &str,
	) -> Result<()>;
	async fn delete_memory(
		&mut self,
		workspace: Uuid,
		agent: String,
		version: String,
		home: String,
	) -> Result<()>;
	async fn change(
		&mut self,
		workspace: Uuid,
		id: Uuid,
		point: Uuid,
		index_revision: i64,
		delete: bool,
		authority: Value,
	) -> Result<Entry>;
	/// Preserve descending candidate pages of 200 and advance past denied rows.
	async fn history_page(&mut self, workspace: Uuid, cursor: i64) -> Result<Vec<History>>;
	async fn history_entry(&mut self, id: Uuid) -> Result<Entry>;
}
