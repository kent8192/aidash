//! Journal transactions preserve database-time fences and reservation history independently of HTTP.
use crate::Result;
use aidash_domain::semantic::remote::{
	Operation, Receipt, SourceRead,
	journal::{Attempt, Record},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait JournalScope: Send {
	async fn insert_binding(
		&mut self,
		operation: &Operation,
		digest: &str,
		value: &Value,
	) -> Result<()>;
	async fn shared(&mut self, id: Uuid) -> Result<Record>;
	async fn locked(&mut self, id: Uuid) -> Result<Record>;
	async fn clock(&mut self) -> Result<DateTime<Utc>>;
	async fn expire_attempt(&mut self, previous: Uuid) -> Result<()>;
	async fn wait_expired(
		&mut self,
		id: Uuid,
		failures: i32,
		delay: Option<i64>,
		state: &str,
		error: &str,
	) -> Result<()>;
	async fn activate(&mut self, id: Uuid, attempt: &Attempt, record: &Record) -> Result<()>;
	async fn current(&mut self, attempt: &Attempt) -> Result<Record>;
	async fn dispatched(&mut self, attempt: &Attempt, reservations: &Value) -> Result<u64>;
	async fn record_source(&mut self, receipt: &Receipt, source: &SourceRead) -> Result<()>;
	async fn complete_operation(&mut self, receipt: &Receipt) -> Result<()>;
	async fn complete_attempt(&mut self, attempt: &Attempt) -> Result<()>;
	async fn fail_operation(
		&mut self,
		record: &Record,
		failures: i32,
		delay: Option<i64>,
		state: &str,
		error: &str,
	) -> Result<()>;
	async fn fail_attempt(&mut self, attempt: &Attempt, error: &str) -> Result<()>;
	async fn resume_records(&mut self, grant: Uuid, admission: Uuid) -> Result<Vec<Record>>;
	async fn resume_record(&mut self, record: &Record) -> Result<()>;
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()>;
	async fn commit(self: Box<Self>) -> Result<()>;
}
#[async_trait]
pub trait JournalRepository: Send + Sync {
	async fn begin(&self) -> Result<Box<dyn JournalScope + '_>>;
	async fn bound(&self, id: Uuid) -> Result<Record>;
	async fn expire_cached(&self, id: Uuid) -> Result<()>;
}
