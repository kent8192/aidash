//! Legacy delegation and discovery share authorization gates with worker delivery.
use crate::{
	Error, Result,
	ports::federation::{FederationRepository, PeerTransport},
};
use aidash_domain::{
	Run, TaskStatus,
	federation::{Delegation, DiscoveredAgent, Discovery, Offer, PeerError},
	qualified_agent,
	registry::{AgentConfig, AgentPage, EntityRef, Entry, Search},
};
use futures_util::{StreamExt, stream};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

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
			400 => Err(
				map_workspace_chunk_bad_request(path, body, &reply.body).unwrap_or_else(|| {
					Error::External(format!("peer {node} returned {}", reply.status_label))
				}),
			),
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
	pub async fn delegate(
		&self,
		task_id: Uuid,
		node: &str,
		agent: &EntityRef,
	) -> Result<Delegation> {
		let task = self.repository.task(task_id).await?;
		self.repository
			.require_legacy_workspace(task.workspace_id)
			.await?;
		if task.status != TaskStatus::Open
			&& task.owner.as_deref() != Some(&qualified_agent(node, &agent.id, &agent.version))
		{
			return Err(Error::Conflict("task is already assigned".into()));
		}
		if node == self.node_id {
			self.repository.require_legacy_agent(agent).await?;
			let entry = self.repository.agent(agent).await?;
			let _: AgentConfig = serde_json::from_value(entry.config.clone())
				.map_err(|_| Error::Invalid("executor must be an agent".into()))?;
			let requirements: Search = serde_json::from_value(task.requirements.clone())?;
			if entry.kind != "agent" || !requirements.matches(&entry) {
				return Err(Error::Invalid(
					"agent does not satisfy task requirements".into(),
				));
			}
		} else {
			let mut requirements: Search = serde_json::from_value(task.requirements.clone())?;
			requirements.kind = Some("agent".into());
			let entry: Entry = self
				.request(
					node,
					"GET",
					&format!("/discover/{}/{}", agent.id, agent.version),
					None,
				)
				.await?;
			if entry.id != agent.id
				|| entry.version != agent.version
				|| !requirements.matches(&entry)
			{
				return Err(Error::Invalid(
					"remote agent is missing or does not satisfy task requirements".into(),
				));
			}
		}
		let mut delegation = self
			.repository
			.reserve_delegation(&task, node, agent)
			.await?;
		match self.deliver(&delegation).await {
			Ok(()) => delegation.delivered = true,
			Err(error) => tracing::warn!(%error,%task_id,"delegation queued for retry"),
		}
		Ok(delegation)
	}
	pub async fn deliver(&self, delegation: &Delegation) -> Result<()> {
		if delegation.delivered {
			return Ok(());
		}
		let task = self.repository.task(delegation.task_id).await?;
		let agent = EntityRef {
			id: delegation.agent_id.clone(),
			version: delegation.agent_version.clone(),
		};
		if delegation.node_id == self.node_id {
			self.repository.accept_local_run(&task, &agent).await?;
		} else {
			self.request::<Run>(
				&delegation.node_id,
				"POST",
				"/offers",
				Some(&json!(Offer { task, agent })),
			)
			.await?;
		}
		self.repository.mark_delivered(delegation.task_id).await
	}
	pub async fn retry_deliveries(&self) -> Result<()> {
		let batch = self.repository.claim_retries().await?;
		let pending = batch.pending().to_vec();
		let mut deliveries = stream::iter(pending.into_iter().map(|delegation| async move {
			let result = self.deliver(&delegation).await;
			(delegation, result)
		}))
		.buffer_unordered(8);
		while let Some((delegation, result)) = deliveries.next().await {
			if let Err(error) = result {
				tracing::warn!(task_id=%delegation.task_id,%error,"peer delivery pending");
			}
		}
		drop(batch);
		Ok(())
	}
}

pub fn map_workspace_chunk_bad_request(
	path: &str,
	request: Option<&Value>,
	error: &Value,
) -> Option<Error> {
	if path != "/workspace"
		|| request.is_none_or(|body| body["operation"] != "workspace_record_chunk")
	{
		return None;
	}
	Some(Error::Invalid(
		error["error"]
			.as_str()
			.unwrap_or("invalid remote workspace record chunk")
			.to_owned(),
	))
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
