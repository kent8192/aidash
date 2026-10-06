//! Federation effects retain native authorization and transaction fences.
use crate::Result;
use aidash_domain::{
	Run, Task,
	federation::{Delegation, Peer},
	registry::{AgentPage, EntityRef, Entry, Search},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

pub struct PeerReply {
	pub status: u16,
	pub status_label: String,
	pub transaction_pending: bool,
	pub run_message_pending: bool,
	pub body: Value,
}

#[async_trait]
pub trait PeerTransport: Send + Sync {
	async fn request(
		&self,
		peer: &Peer,
		method: &str,
		path: &str,
		body: Option<&Value>,
	) -> Result<PeerReply>;
}

/// A claimed retry batch owns its visibility lease until all deliveries finish.
/// Dropping an interrupted batch releases that lease without undoing committed
/// retry deadlines. Repeated delivery uses the original task and agent identity.
pub trait DelegationRetries: Send {
	fn pending(&self) -> &[Delegation];
}

#[async_trait]
pub trait FederationRepository: Send + Sync {
	async fn peers(&self) -> Result<Vec<Peer>>;
	async fn peer(&self, node: &str) -> Result<Peer>;
	async fn task(&self, id: Uuid) -> Result<Task>;
	async fn require_legacy_workspace(&self, workspace: Uuid) -> Result<()>;
	async fn require_legacy_agent(&self, agent: &EntityRef) -> Result<()>;
	async fn agent(&self, agent: &EntityRef) -> Result<Entry>;
	async fn agents(&self, search: &Search, offset: u64) -> Result<AgentPage>;
	/// Atomically recheck the task revision and claimant, reserve its delegation,
	/// increment the task revision, and append the audit/outbox event. A scoped
	/// workspace must fail the legacy authorization check inside this boundary.
	async fn reserve_delegation(
		&self,
		expected: &Task,
		node: &str,
		agent: &EntityRef,
	) -> Result<Delegation>;
	async fn accept_local_run(&self, task: &Task, agent: &EntityRef) -> Result<Run>;
	async fn mark_delivered(&self, task: Uuid) -> Result<()>;
	/// Claim using the database clock, skip locked rows, retain visibility, and
	/// atomically advance retry deadlines before exposing this batch.
	async fn claim_retries(&self) -> Result<Box<dyn DelegationRetries>>;
}

pub mod dependencies;

pub mod foreign_reads;

pub mod registry_reads;

pub mod authority;

pub mod run_messages;

pub mod peers;
