//! Current authority admission precedes a separate owned usage transaction.
use crate::{Result, ports::catalog::CatalogScope};
use aidash_domain::{
	generation::{
		remote::{Attempt, ReservationBinding, Usage},
		requests::Request,
	},
	policy::Resource,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

/// Every read and permission check borrows the caller's current authority lease.
#[async_trait]
pub trait GenerationUsageAuthority: Send {
	fn now(&self) -> DateTime<Utc>;
	/// Current subjects determine these rows, sorted by request ID.
	async fn jobs(&mut self, node: &str) -> Result<Vec<Request>>;
	/// Retain the current policy share lock and full specification decoding.
	async fn policy_enabled(&mut self, job: &Request) -> Result<bool>;
	async fn pinned_policy(&mut self, job: &Request) -> Result<Value>;
	fn catalog(&mut self) -> &mut dyn CatalogScope;
	fn remote_resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
}

/// Budget locks and conditional updates are held until this session commits.
/// Drop rolls back all ancestor debits on failure or cancellation.
#[async_trait]
pub trait GenerationReservationSession: Send {
	async fn lock_attempt(&mut self, attempt: Uuid, digest: &str) -> Result<Attempt>;
	async fn lock_budget(&mut self, job: &Request) -> Result<()>;
	async fn existing(
		&mut self,
		job: &Request,
		usage: &Usage,
	) -> Result<Option<ReservationBinding>>;
	/// Debit tokens and any purpose call counter with their bounds in one CAS.
	async fn debit_budget(&mut self, job: &Request, usage: &Usage) -> Result<u64>;
	async fn insert(&mut self, job: &Request, usage: &Usage, digest: &str) -> Result<()>;
	async fn commit(self: Box<Self>) -> Result<()>;
}
#[async_trait]
pub trait GenerationReservationRepository: Send + Sync {
	fn node_id(&self) -> &str;
	async fn begin(&self) -> Result<Box<dyn GenerationReservationSession>>;
}
