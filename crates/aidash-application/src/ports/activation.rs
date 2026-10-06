//! Native scopes retain visibility, row-lock ordering and atomic handoff writes.
use crate::Result;
use aidash_domain::{
	Run, RunMetadata, RunState,
	activation::{Envelope, Obligation, QuarantineReason},
	run_state::RawRun,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use uuid::Uuid;

pub type RecoveryCursor = Option<(DateTime<Utc>, Uuid)>;
pub struct Approval {
	pub state: String,
	pub expires_at: Option<DateTime<Utc>>,
}

#[async_trait]
pub trait SchedulingScope: Send {
	async fn now(&mut self) -> Result<DateTime<Utc>>;
	/// Oldest first, at most 128 rows, FOR UPDATE SKIP LOCKED; target is exact.
	async fn candidates(
		&mut self,
		target: Option<Uuid>,
		cursor: RecoveryCursor,
	) -> Result<Vec<RawRun>>;
	async fn unblocked(&mut self, run: Uuid) -> Result<bool>;
	async fn human_answered(&mut self, id: Uuid) -> Result<bool>;
	async fn approval(&mut self, id: Uuid) -> Result<Option<Approval>>;
	async fn dependencies_ready(&mut self, task: Uuid) -> Result<bool>;
	/// Compare revision, install the lease, advance revision and persist recovery state.
	async fn lease(&mut self, run: &Run, token: Uuid, seconds: i32) -> Result<Option<Run>>;
	/// Repair first acquires a fenced lease; a row lock alone does not authorize a write.
	async fn repair_lease(
		&mut self,
		run: &RunMetadata,
		token: Uuid,
		seconds: i32,
	) -> Result<Option<RunMetadata>>;
	async fn invalid_state(
		&mut self,
		owned: &RunMetadata,
		token: Uuid,
		state: &RunState,
		reason: &str,
		node: &str,
	) -> Result<()>;
}

#[async_trait]
pub trait ClaimScope: SchedulingScope {
	/// This lock always precedes the obligation lock; NOWAIT preserves visibility retry.
	async fn lock_run(&mut self, id: Uuid) -> Result<Option<RawRun>>;
	async fn read_run(&mut self, id: Uuid) -> Result<RawRun>;
	async fn lock_obligation(&mut self, id: Uuid) -> Result<Option<Obligation>>;
	async fn newer_transition(&mut self, obligation: &Obligation, run: &RawRun) -> Result<bool>;
	async fn settle(&mut self, id: Uuid, reason: &str) -> Result<()>;
	async fn record_claim(&mut self, id: Uuid, run: &Run, token: Uuid, seconds: i32) -> Result<()>;
	async fn defer(&mut self, id: Uuid, due: Option<DateTime<Utc>>) -> Result<()>;
	async fn commit(self: Box<Self>) -> Result<()>;
}

#[async_trait]
pub trait VisibilityScope: Send {
	async fn suspend(&mut self) -> Result<()>;
	/// Execution consumes the retained lease and applies normal worker authorization.
	async fn advance(self: Box<Self>, run: Run, token: Uuid) -> Result<()>;
}

#[async_trait]
pub trait ActivationRepository: Send + Sync {
	fn node_id(&self) -> &str;
	fn lease_seconds(&self) -> i32;
	async fn visibility(&self) -> Result<Box<dyn VisibilityScope>>;
	async fn claim_scope(&self) -> Result<Box<dyn ClaimScope>>;
	async fn recover(&self) -> Result<Option<(Run, Uuid)>>;
	/// One batch, protected by the ordinary-state visibility barrier.
	async fn reconcile(&self) -> Result<u64>;
	async fn publish_batch(&self, token: Uuid) -> Result<Vec<Obligation>>;
	async fn published(&self, row: &Obligation, token: Uuid) -> Result<()>;
	async fn quarantine(
		&self,
		payload: &[u8],
		reason: QuarantineReason,
		sequence: Option<u64>,
	) -> Result<()>;
	async fn observe(&self) -> Result<()>;
}

#[async_trait]
pub trait ActivationDelivery: Send + Sync {
	fn payload(&self) -> &[u8];
	fn sequence(&self) -> Option<u64>;
	async fn acknowledge(&self) -> Result<()>;
	async fn defer(&self) -> Result<()>;
	async fn discard(&self) -> Result<()>;
}

#[async_trait]
pub trait ActivationTransport: Send + Sync {
	fn disconnected(&self) -> bool;
	fn connected(&self) -> bool;
	async fn publish(&self, envelope: &Envelope, epoch: i64) -> Result<bool>;
	/// Fetch exactly one reference in a finite batch, with no speculative prefetch.
	async fn fetch(&self) -> Result<Option<Box<dyn ActivationDelivery>>>;
}

#[async_trait]
pub trait ActivationConnector: Send + Sync {
	async fn connect(&self) -> Result<Arc<dyn ActivationTransport>>;
}
