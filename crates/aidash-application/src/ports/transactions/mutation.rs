//! All mutation reads, writes and events borrow one caller-owned transaction.
use crate::{Result, ports::registry::DefinitionWriter};
use aidash_domain::{Artifact, ArtifactInput, Task, Workspace, run_state::RawRun};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait MutationScope: DefinitionWriter {
	async fn replace_workspace(
		&mut self,
		id: Uuid,
		revision: i64,
		state: Value,
	) -> Result<Workspace>;
	/// Retain the row lock until the caller settles its transaction.
	async fn lock_task(&mut self, id: Uuid) -> Result<Task>;
	async fn unfinished_children(&mut self, id: Uuid) -> Result<bool>;
	async fn delegated_node(&mut self, task: Uuid) -> Result<Option<String>>;
	/// Update only completion columns, keeping topology and its separate lock intact.
	async fn complete_task(
		&mut self,
		task: &Task,
		artifact: &ArtifactInput,
		key: &str,
	) -> Result<(Task, Artifact)>;
	async fn source_run(&mut self, task: Uuid) -> Result<Option<Uuid>>;
	async fn record_output(
		&mut self,
		run: Uuid,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<()>;
	/// Return undecoded state and the database-clock lease result under the row lock.
	async fn lock_run(&mut self, id: Uuid) -> Result<(RawRun, bool)>;
	async fn complete_run(&mut self, id: Uuid) -> Result<()>;
	async fn append_event(
		&mut self,
		node: &str,
		workspace: Option<Uuid>,
		kind: &str,
		data: Value,
	) -> Result<()>;
}
