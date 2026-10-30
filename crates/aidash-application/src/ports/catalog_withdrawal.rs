//! What withdrawing one Catalog Approval would affect on one Node.
use crate::Result;
use aidash_domain::{
	nodes::NodeAddress,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;

/// Everything a withdrawal impact reads. The caller owns the transaction and
/// the Node selection; reading the impact never writes.
#[async_trait]
pub trait WithdrawalImpactScope: Send {
	fn node(&self) -> &NodeAddress;
	/// The raw exact version, approved or not.
	async fn definition(&mut self, reference: &EntityRef) -> Result<Option<Entry>>;
	/// `catalog.approve` on the definition's approval resource, the check a
	/// withdrawal itself makes.
	async fn can_approve(&mut self, entry: &Entry) -> Result<bool>;
	/// The agent definitions whose approval is currently enabled on the Node.
	async fn approved_agents(&mut self) -> Result<Vec<Entry>>;
	/// How many non-terminal Runs of these exact agent versions the Node executes.
	async fn non_terminal_runs(&mut self, agents: &[EntityRef]) -> Result<u64>;
}
