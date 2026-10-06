//! Home intent sessions keep credential, policy and intent locks through commit.
use super::ForeignGenerationGuard;
use crate::{Error, Result, authorization::Snapshot};
use aidash_domain::{
	Task,
	generation::intent::{Intent, Prepared, guards::Record},
	identity::Principal,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[async_trait]
pub trait HomeGenerationScope: ForeignGenerationGuard {
	fn replace_subjects(&mut self, subjects: Vec<String>);
	fn snapshot(&self) -> &Snapshot;
	fn snapshot_mut(&mut self) -> &mut Snapshot;
	async fn inherit_task_origin(&mut self, id: Uuid) -> Result<()>;
	async fn task(&mut self, id: Uuid) -> Result<Task>;
	async fn insert(&mut self, intent: &Intent) -> Result<()>;
	/// Idempotency reads use the original transaction without an extra row lock.
	async fn saved(&mut self, id: Uuid) -> Result<Record>;
	/// Unlike optional inspection, a missing row remains a repository error.
	async fn current(&mut self, id: Uuid) -> Result<Record>;
	async fn save_snapshot(&mut self) -> Result<()>;
	async fn set_cancelled(&mut self, id: Uuid) -> Result<()>;
	async fn finish(self: Box<Self>) -> Result<()>;
	/// Roll back protected effects and preserve durable denial audits and errors.
	async fn abort(self: Box<Self>, error: Error) -> Error;
}

#[async_trait]
pub trait HomeGenerationRepository: Send + Sync {
	fn principal(&self) -> &Principal;
	fn node_id(&self) -> &str;
	fn credential_id(&self) -> Option<Uuid>;
	fn now(&self) -> DateTime<Utc>;
	async fn load(&self, id: Uuid) -> Result<Record>;
	async fn begin(&self) -> Result<Box<dyn HomeGenerationScope>>;
	/// Saved durable authority intentionally carries no browser session.
	async fn begin_saved(
		&self,
		record: &Record,
		exclusive: bool,
	) -> Result<Box<dyn HomeGenerationScope>>;
	/// Called after committing the Home intent, without holding a local lease.
	async fn prepare(&self, target: &str, id: Uuid) -> Result<Prepared>;
}
