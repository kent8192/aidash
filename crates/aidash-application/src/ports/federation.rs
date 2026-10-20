//! Federation effects retain native authorization and transaction fences.
use crate::Result;
use aidash_domain::federation::route::Route;
use async_trait::async_trait;
use serde_json::Value;

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
		route: &Route,
		method: &str,
		path: &str,
		body: Option<&Value>,
	) -> Result<PeerReply>;
}

#[async_trait]
pub trait FederationRepository: Send + Sync {
	/// The way to `node` for the sender this repository was built for: its
	/// enabled Node Registration of the target, or the target itself when it
	/// is another Node of the sender's own Tenant.
	async fn route(&self, node: &str) -> Result<Route>;
	/// Every Node the sender addresses: the Nodes registered at it and the
	/// other Nodes of its own Tenant.
	async fn routes(&self) -> Result<Vec<Route>>;
}

pub mod dependencies;

pub mod foreign_reads;

pub mod registry_reads;

pub mod authority;

pub mod run_messages;

pub mod inbound;

pub mod registrations;
