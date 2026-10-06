//! Native management records and authority calls share the existing bootstrap.
use crate::Result;
use aidash_domain::{
	federation::Peer,
	identity::execution::ExecutionPrincipal,
	transactions::{
		Manifest,
		authority::Status,
		coordination::{LocalStatus, Vote},
		management::{History, Trust},
	},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Retain the connection registration through the post-commit pending scan.
/// Setting trust commits its own advisory-locked transaction and audit atomically.
#[async_trait]
pub trait TrustScope: Send {
	async fn set(&mut self, trust: Trust) -> Result<Trust>;
	async fn pending_peer(&mut self, node: &str) -> Result<Vec<Uuid>>;
}

#[async_trait]
pub trait ManagementRepository: Send + Sync {
	async fn submit_operator(&self, manifest: &Manifest) -> Result<Status>;
	async fn submit_subject(
		&self,
		identity: &ExecutionPrincipal,
		manifest: &Manifest,
	) -> Result<Status>;
	/// Return ordered keyset pages of at most 200 durable ownership candidates.
	async fn candidates(
		&self,
		identity: Option<&ExecutionPrincipal>,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<Status>>;
	async fn require_owner(&self, identity: &ExecutionPrincipal, id: Uuid) -> Result<()>;
	/// Delegate to the shared application authority workflow, preserving its scope.
	/// Transport unavailability retains the portable External error category.
	async fn authorize(
		&self,
		identity: &ExecutionPrincipal,
		state: &Status,
		action: &str,
	) -> Result<()>;
	async fn status(&self, id: Uuid) -> Result<Status>;
	/// Read votes then audit while retaining the same connection registration.
	async fn history(&self, id: Uuid) -> Result<(Vec<Vote>, Vec<History>)>;
	async fn abort_operator(&self, id: Uuid) -> Result<Status>;
	async fn participants(&self) -> Result<Vec<LocalStatus>>;
	async fn trusts(&self) -> Result<Vec<Trust>>;
	async fn ensure_peer(&self, node: &str) -> Result<()>;
	async fn trust_scope(&self) -> Result<Box<dyn TrustScope>>;
	async fn pending_peer(&self, node: &str) -> Result<Vec<Uuid>>;
	fn credential(&self, reference: &str) -> Result<String>;
	async fn restore_peer(&self, node: &str, reference: &str, credential: &str) -> Result<Peer>;
}
