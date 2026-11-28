//! All mutation reads, writes and events borrow one caller-owned transaction.
use crate::Result;
use aidash_domain::{Message, Task, Thread, run_state::RawRun};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait MutationScope: Send {
	async fn replace_thread(&mut self, id: Uuid, revision: i64, title: String) -> Result<Thread>;
	/// Retain the row lock until the caller settles its transaction.
	async fn lock_task(&mut self, id: Uuid) -> Result<Task>;
	async fn unfinished_children(&mut self, id: Uuid) -> Result<bool>;
	async fn delegated_node(&mut self, task: Uuid) -> Result<Option<String>>;
	/// Update only completion columns, keeping topology and its separate lock intact.
	async fn complete_task(
		&mut self,
		task: &Task,
		text: &str,
		key: &str,
	) -> Result<(Task, Message)>;
	async fn source_run(&mut self, task: Uuid) -> Result<Option<Uuid>>;
	async fn record_output(&mut self, run: Uuid, thread: Uuid, kind: &str, id: Uuid) -> Result<()>;
	/// Return undecoded state and the database-clock lease result under the row lock.
	async fn lock_run(&mut self, id: Uuid) -> Result<(RawRun, bool)>;
	async fn complete_run(&mut self, id: Uuid) -> Result<()>;
	/// Lock what finishing `run` ends (see `subnodes::lock_end`) before any
	/// event of this transaction is appended. Locks nothing by default.
	async fn lock_subnode_end(&mut self, _run: Uuid) -> Result<()> {
		Ok(())
	}
	async fn append_event(&mut self, thread: Option<Uuid>, kind: &str, data: Value) -> Result<()>;
}
