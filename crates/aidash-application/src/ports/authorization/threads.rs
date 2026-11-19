//! Thread ownership and interaction borrow current authority and existing row locks.
use crate::{Result, ports::authorization::lease::AuthorizationLease};
use aidash_domain::{Run, RunMetadata, policy::Resource};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
/// What a Thread's stored authority row says about it: who owns it and, for a
/// Duty's own Thread, which Duty (#1008, ADR 0140). Every owner lookup returns
/// both, from the same row read, so the `thread` resource has the same
/// attributes on a single read, in a list and in the Node administrator check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadOwner {
	pub owner: String,
	/// The Duty whose own Thread this is. `None` for every other Thread,
	/// including the Thread a Duty was registered from and the shared Thread a
	/// Duty registered before #1008 still uses.
	pub duty_id: Option<Uuid>,
}
impl ThreadOwner {
	/// A Thread that belongs to no Duty.
	pub fn person(owner: impl Into<String>) -> Self {
		Self {
			owner: owner.into(),
			duty_id: None,
		}
	}
}
#[async_trait]
pub trait ThreadResourceScope: Send {
	fn inherited(&self) -> bool;
	fn context(&self) -> &Value;
	/// Whether a Read Recheck in progress has opened `id` for this authority.
	fn recheck_reaches(&self, _id: Uuid) -> bool {
		false
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn owner(&mut self, id: Uuid, lock: bool) -> Result<Option<ThreadOwner>>;
}
#[async_trait]
pub trait ThreadAuthorityScope: ThreadResourceScope + AuthorizationLease {
	async fn locked_owner(&mut self, id: Uuid) -> Result<Option<ThreadOwner>>;
	fn identity(&self) -> (&str, &str);
	async fn all_owners(&mut self) -> Result<Vec<(Uuid, ThreadOwner)>>;
	async fn event_owners(&mut self, selected: Option<Uuid>) -> Result<Vec<(Uuid, ThreadOwner)>>;
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
