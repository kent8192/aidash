//! Usage settlement retains the same owned attempt and budget transaction.
use crate::Result;
use aidash_domain::generation::{
	remote::{Attempt, Purpose, ReservedCharge},
	requests::Request,
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait GenerationSettlementScope: Send {
	/// Insert idempotently and lock the attempt before any usage row operation.
	async fn lock_attempt(&mut self, attempt: Uuid, digest: &str) -> Result<Attempt>;
	/// Return matching rows sorted by request ID, holding update locks throughout.
	async fn reservations(&mut self, attempt: Uuid, digest: &str) -> Result<Vec<ReservedCharge>>;
	/// Keep token/call decrements and their nonnegative predicates in one CAS.
	async fn refund_budget(
		&mut self,
		request: Uuid,
		tokens: i64,
		purpose: Purpose,
		release_call: bool,
	) -> Result<u64>;
	async fn request(&mut self, id: Uuid) -> Result<Request>;
	/// Retain the policy allocation CAS when terminal lifecycle released quota.
	async fn refund_policy(
		&mut self,
		job: &Request,
		tokens: i64,
		purpose: Purpose,
		release_call: bool,
	) -> Result<u64>;
	async fn settle_reservation(
		&mut self,
		request: Uuid,
		attempt: Uuid,
		state: &str,
		reported: Option<i64>,
	) -> Result<()>;
	async fn save_finalization(&mut self, attempt: Uuid, result: &Value) -> Result<()>;
}

/// Drop rolls back every provisional refund and decision on error/cancellation.
#[async_trait]
pub trait GenerationSettlementSession: GenerationSettlementScope {
	async fn commit(self: Box<Self>) -> Result<()>;
}

#[async_trait]
pub trait GenerationSettlementRepository: Send + Sync {
	async fn begin(&self) -> Result<Box<dyn GenerationSettlementSession>>;
}
