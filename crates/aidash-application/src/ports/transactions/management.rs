//! Native management records and authority calls share the existing bootstrap.
use crate::Result;
use aidash_domain::{
	identity::execution::ExecutionPrincipal,
	transactions::{
		Manifest,
		authority::Status,
		coordination::{LocalStatus, Vote},
		management::History,
	},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[async_trait]
pub trait ManagementRepository: Send + Sync {
	async fn submit_subject(
		&self,
		identity: &ExecutionPrincipal,
		manifest: &Manifest,
	) -> Result<Status>;
	/// Return ordered keyset pages of at most 200 durable ownership candidates.
	async fn candidates(
		&self,
		identity: &ExecutionPrincipal,
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
	async fn participants(&self) -> Result<Vec<LocalStatus>>;
}
