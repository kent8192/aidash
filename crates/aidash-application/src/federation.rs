//! Operator Agent discovery across enabled peers.
use crate::{
	Error, Result,
	ports::federation::{FederationRepository, PeerTransport},
};
use aidash_domain::{
	federation::{DiscoveredAgent, Discovery, PeerError},
	registry::{AgentPage, Entry, Search},
};
use futures_util::{StreamExt, stream};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::sync::Arc;

pub struct Federation {
	node_id: String,
	repository: Arc<dyn FederationRepository>,
	transport: Arc<dyn PeerTransport>,
}
impl Federation {
	pub fn new(
		node_id: String,
		repository: Arc<dyn FederationRepository>,
		transport: Arc<dyn PeerTransport>,
	) -> Self {
		Self {
			node_id,
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
	async fn discovery_entries(&self, node: &str, search: &Search) -> Result<Vec<Entry>> {
		let mut offset = 0;
		let mut entries = Vec::new();
		loop {
			let page = if node == self.node_id {
				self.repository.agents(search, offset).await?
			} else {
				self.request::<AgentPage>(
					node,
					"POST",
					&format!("/discover?offset={offset}"),
					Some(&json!(search)),
				)
				.await?
			};
			entries.extend(page.entries);
			match page.next_offset {
				Some(next) if next > offset => offset = next,
				Some(_) => return Err(Error::External("discovery cursor did not advance".into())),
				None => return Ok(entries),
			}
		}
	}
	pub async fn discover(&self, search: &Search) -> Result<Discovery> {
		let mut query = search.clone();
		query.kind = Some("agent".into());
		let mut result = Discovery {
			agents: self
				.discovery_entries(&self.node_id, &query)
				.await?
				.into_iter()
				.map(|entity| DiscoveredAgent {
					node_id: self.node_id.clone(),
					entity,
				})
				.collect(),
			errors: Vec::new(),
		};
		let peers = self
			.repository
			.peers()
			.await?
			.into_iter()
			.filter(|peer| peer.enabled);
		let mut replies = stream::iter(peers.map(|peer| {
			let query = &query;
			async move {
				let reply = self.discovery_entries(&peer.node_id, query).await;
				(peer, reply)
			}
		}))
		.buffer_unordered(8);
		while let Some((peer, reply)) = replies.next().await {
			match reply {
				Ok(entries) => result.agents.extend(
					entries
						.into_iter()
						.filter(|entry| query.matches(entry))
						.map(|entity| DiscoveredAgent {
							node_id: peer.node_id.clone(),
							entity,
						}),
				),
				Err(error) => result.errors.push(PeerError {
					node_id: peer.node_id,
					error: error.to_string(),
				}),
			}
		}
		Ok(result)
	}
}

#[cfg(test)]
mod tests;

pub mod dependencies;

pub mod admission;
pub mod foreign_reads;

pub mod graph;

pub mod registry_reads;

pub mod authority;

pub mod run_messages;

pub mod peers;
