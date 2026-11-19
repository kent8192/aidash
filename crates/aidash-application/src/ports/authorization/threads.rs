//! Thread ownership and interaction borrow current authority and existing row locks.
use crate::{Result, ports::authorization::lease::AuthorizationLease};
use aidash_domain::{Run, RunMetadata, policy::Resource};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait ThreadResourceScope: Send {
	fn inherited(&self) -> bool;
	fn context(&self) -> &Value;
	/// Whether a Read Recheck in progress has opened `id` for this authority.
	fn recheck_reaches(&self, _id: Uuid) -> bool {
		false
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn owner(&mut self, id: Uuid, lock: bool) -> Result<Option<String>>;
}
#[async_trait]
pub trait ThreadAuthorityScope: ThreadResourceScope + AuthorizationLease {
	async fn locked_owner(&mut self, id: Uuid) -> Result<Option<String>>;
	fn identity(&self) -> (&str, &str);
	async fn all_owners(&mut self) -> Result<Vec<(Uuid, String)>>;
	async fn event_owners(&mut self, selected: Option<Uuid>) -> Result<Vec<(Uuid, String)>>;
	/// The Node the Thread belongs to: whoever administers that Node reads
	/// every Thread in it (#1008, ADR 0140).
	async fn thread_node(&mut self, id: Uuid) -> Result<String>;
}
#[async_trait]
pub trait RunInteractionScope: ThreadResourceScope {
	async fn run(&mut self, id: Uuid) -> Result<Option<Run>>;
	fn set_context(&mut self, context: Value);
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn run_visible(&mut self, run: &RunMetadata) -> Result<bool>;
	/// Whether this Node holds an admission for the Run, that is, executes it for another Home.
	async fn admitted_here(&mut self, run: &RunMetadata) -> Result<bool>;
}
