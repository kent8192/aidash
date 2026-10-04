//! Participant scopes own the exact transaction through validation and settlement.
use crate::Result;
use aidash_domain::transactions::{
	Manifest,
	coordination::{LocalStatus, ParticipantPhase},
};
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait ParticipantScope: Send {
	async fn lock(&mut self, id: Uuid) -> Result<Option<LocalStatus>>;
	async fn trusted(&mut self, node: &str) -> Result<bool>;
	async fn gate_exclusive(&mut self) -> Result<Option<Uuid>>;
	async fn reserve_gate(&mut self, id: Uuid) -> Result<()>;
	async fn release_gate(&mut self, committed: bool) -> Result<()>;
	async fn insert(&mut self, manifest: &Manifest, phase: ParticipantPhase)
	-> Result<LocalStatus>;
	async fn transition(&mut self, id: Uuid, phase: ParticipantPhase) -> Result<LocalStatus>;
	async fn reservation_history(&mut self, id: Uuid) -> Result<()>;
	/// Use the same mutation implementation under a rollback-only savepoint.
	/// Session mutation context is established before the savepoint and retained.
	async fn validate_mutations(&mut self, manifest: &Manifest) -> Result<()>;
	/// Apply mutations and their events in this transaction under its visibility gate.
	async fn apply_mutations(&mut self, manifest: &Manifest) -> Result<()>;
	/// Success commits. Failure preserves authority auditing or drops the standalone
	/// transaction for RAII rollback; error identity must survive the boundary.
	async fn finish(self: Box<Self>, result: Result<LocalStatus>) -> Result<LocalStatus>;
	/// Explicitly roll back an empty durable-admission probe before live authority.
	async fn rollback(self: Box<Self>) -> Result<()>;
}

pub trait PendingParticipants: Send + Sync {
	fn rows(&self) -> &[LocalStatus];
}

#[async_trait]
pub trait ParticipantRepository: Send + Sync {
	fn node_id(&self) -> &str;
	/// Open a native SERIALIZABLE transaction with physical connection affinity.
	async fn begin(&self) -> Result<Box<dyn ParticipantScope>>;
	/// Retain current authority locks in the returned scope's original transaction.
	async fn admission(
		&self,
		caller: &str,
		manifest: &Manifest,
	) -> Result<Option<Box<dyn ParticipantScope>>>;
	async fn pending(&self) -> Result<Box<dyn PendingParticipants>>;
	async fn fault(&self, id: Uuid, point: &str) -> Result<()>;
	fn wake(&self);
}
