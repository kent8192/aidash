//! One Home/executor ledger scope preserves atomic import and durable message fences.
use crate::Result;
use aidash_domain::{Message, RunMetadata, SnapshotPage, run_input::RunInput};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait RunMessages: Send + Sync {
	fn node(&self) -> &str;
	fn run(&self) -> &RunMetadata;
	fn local(&self) -> bool {
		self.run().home_node == self.node()
	}
	async fn inputs(&self) -> Result<Vec<RunInput>>;
	async fn has_media(&self, message: Uuid) -> Result<bool>;
	async fn sequence(&self, key: &str, content: &str) -> Result<i64>;
	async fn input_limit(&self) -> Result<usize>;
	async fn accept(&self, sender: &str, content: &str, key: &str, limit: usize) -> Result<()>;
	/// Import Home history and accept the new correction in the same transaction.
	async fn import_and_accept(
		&self,
		history: &[(String, Message)],
		sender: &str,
		content: &str,
		key: &str,
		limit: usize,
	) -> Result<()>;
	async fn import_history(&self, history: &[(String, Message)], limit: usize) -> Result<()>;
	async fn bind(&self, key: &str, message: Uuid) -> Result<()>;
	async fn reserve(&self, key: &str, content: &str) -> Result<bool>;
	async fn promote(&self, key: &str, content: &str, sequence: i64) -> Result<bool>;
	async fn release(&self, keys: &[String]) -> Result<()>;
	async fn acknowledge(&self, keys: &[String]) -> Result<()>;
	async fn delivery(&self, key: &str, content: &str) -> Result<Option<Message>>;
	async fn legacy_message(&self, key: &str, content: &str) -> Result<()>;
	async fn snapshot_page(&self, after: Option<Uuid>) -> Result<SnapshotPage>;
	async fn history_page(&self, offset: usize) -> Result<Option<Vec<Message>>>;
	async fn capability(&self) -> Result<Option<Value>>;
}
