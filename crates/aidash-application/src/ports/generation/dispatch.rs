//! Dispatch ports preserve admission CAS, durable decisions and visibility leases.
use crate::Result;
use aidash_domain::generation::{
	dispatch::{FinalizeInput, Input, Record},
	remote::{Finalization, Reserved, Usage},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

/// This owned transaction rolls back on error, cancellation or panic.
#[async_trait]
pub trait DispatchPreparation: Send {
	async fn insert(&mut self, input: &Input, peer: &str, digest: &str) -> Result<()>;
	/// Acquire a shared row lock before returning the existing admission binding.
	async fn record(&mut self, attempt: Uuid) -> Result<Record>;
	async fn commit(self: Box<Self>) -> Result<()>;
}

/// Keep node-wide visibility protected until explicitly suspended or dropped.
#[async_trait]
pub trait DispatchVisibility: Send {
	async fn terminal_record(&mut self, attempt: Uuid) -> Result<Option<Record>>;
	/// Atomically abort only PREPARING rows older than 120 database-clock seconds.
	/// An aborted row can no longer cross the admission CAS.
	async fn abort_stale_preparations(&mut self) -> Result<()>;
	/// Return at most 16 terminal, unacknowledged attempts, ordered by creation.
	async fn pending(&mut self) -> Result<Vec<Uuid>>;
	async fn suspend(&mut self) -> Result<()>;
}

#[async_trait]
pub trait GenerationDispatchRepository: Send + Sync {
	fn node_id(&self) -> &str;
	async fn begin_preparation(&self) -> Result<Box<dyn DispatchPreparation>>;
	async fn record(&self, attempt: Uuid) -> Result<Option<Record>>;
	/// Change exactly the matching digest's PREPARING row to DISPATCHED.
	async fn admit(&self, input: &Input, receipts: &[Reserved]) -> Result<u64>;
	/// Persist the decision only when the existing state matches the expected state.
	async fn finalize(&self, attempt: Uuid, from: &str, state: &str, result: &Value)
	-> Result<u64>;
	async fn begin_visibility(&self) -> Result<Box<dyn DispatchVisibility>>;
	async fn mark_peer_finalized(&self, attempt: Uuid) -> Result<()>;
}

/// Local allowance settlement and the authenticated, bounded authority RPC.
/// Neither operation contacts a model provider or dispatches another inference.
#[async_trait]
pub trait GenerationDispatchSettlement: Send + Sync {
	async fn local(&self, usage: &Usage, result: &Finalization) -> Result<()>;
	/// Preserve the existing boolean wire response, including a false response.
	/// Transport, authentication and decoding failures remain distinguishable.
	async fn peer(&self, node: &str, input: &FinalizeInput) -> Result<bool>;
}
