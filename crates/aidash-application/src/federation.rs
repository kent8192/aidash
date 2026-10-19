//! Requests to enabled peers, with replies classified for retries.
use crate::{
	Error, Result,
	ports::federation::{FederationRepository, PeerTransport},
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::Arc;

pub struct Federation {
	repository: Arc<dyn FederationRepository>,
	transport: Arc<dyn PeerTransport>,
}
impl Federation {
	pub fn new(
		repository: Arc<dyn FederationRepository>,
		transport: Arc<dyn PeerTransport>,
	) -> Self {
		Self {
			repository,
			transport,
		}
	}
	pub async fn request<T: DeserializeOwned>(
		&self,
		node: &str,
		method: &str,
		path: &str,
		body: Option<&Value>,
	) -> Result<T> {
		let peer = self.repository.peer(node).await?;
		let reply = self.transport.request(&peer, method, path, body).await?;
		match reply.status {
			200..=299 => serde_json::from_value(reply.body)
				.map_err(|error| Error::External(format!("invalid response JSON: {error}"))),
			503 if reply.transaction_pending => Err(Error::TransactionPending),
			409 if reply.run_message_pending => Err(Error::TransactionPending),
			409 => Err(Error::Conflict("remote task state changed".into())),
			_ => Err(Error::External(format!(
				"peer {node} returned {}",
				reply.status_label
			))),
		}
	}
}

#[cfg(test)]
mod tests;

pub mod dependencies;

pub mod admission;
pub mod foreign_reads;

pub mod graph;

pub mod key;

pub mod registry_reads;

pub mod authority;

pub mod run_messages;

pub mod peers;
