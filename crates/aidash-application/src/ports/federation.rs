//! Federation effects retain native authorization and transaction fences.
use crate::Result;
use aidash_domain::federation::Peer;
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
		peer: &Peer,
		method: &str,
		path: &str,
		body: Option<&Value>,
	) -> Result<PeerReply>;
}

#[async_trait]
pub trait FederationRepository: Send + Sync {
	async fn peers(&self) -> Result<Vec<Peer>>;
	async fn peer(&self, node: &str) -> Result<Peer>;
}

pub mod dependencies;

pub mod foreign_reads;

pub mod registry_reads;

pub mod authority;

pub mod run_messages;

pub mod peers;

pub mod registrations;
