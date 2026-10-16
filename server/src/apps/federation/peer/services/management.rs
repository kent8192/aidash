//! Management use cases.
use crate::{
	Error, Result,
	apps::federation::peer::serializers::mesh::MeshNode,
	apps::federation::peer::serializers::mesh::MeshResponse,
	apps::federation::peer::serializers::mesh::PeerError,
	apps::workspaces::serializers::tasks::PageQuery,
	authorization::{execution, identity::Actor},
	domain::*,
	federation::{Discovery, Federation, Offer, Peer},
	registry::{EntityRef, Entry, Search},
	tool::required,
};
use futures_util::{StreamExt, stream};
use http::HeaderMap;
use reinhardt::injectable;
use reinhardt::query::Alias;
use reinhardt::query::OnConflict;
use reinhardt::query::PostgresQueryBuilder;
use reinhardt::query::Query;
use reinhardt::query::QueryStatementBuilder as _;
use reinhardt::query::{Expr, ExprTrait, LockType};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::apps::identity::services::http_auth::peer_node;
#[derive(Clone)]
pub struct PeerManagement {
	runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> PeerManagement {
	PeerManagement { runtime }
}
impl PeerManagement {
	pub(crate) async fn peer_create(&self, peer: Peer) -> Result<Peer> {
		let f = self.runtime.clone();
		f.register_peer(peer).await
	}
	pub(crate) async fn discover(&self, actor: Actor, query: Search) -> Result<Discovery> {
		let f = self.runtime.clone();
		if let Actor::Subject(identity) = actor {
			return execution::discover(&f, &identity, &query).await;
		}
		f.discover(&query).await
	}
	pub(crate) async fn mesh(&self) -> Result<MeshResponse> {
		let f = self.runtime.clone();
		let mut nodes = Vec::new();
		let mut errors = Vec::new();
		let peers = f.peers().await?.into_iter().filter(|p| p.enabled);
		let f = &f;
		let mut responses = stream::iter(peers.map(|peer| async move {
			let response = f
				.request::<MeshNode>(&peer.node_id, reqwest::Method::GET, "/observe", None)
				.await;
			(peer, response)
		}))
		.buffer_unordered(8);
		while let Some((peer, response)) = responses.next().await {
			match response {
				Ok(data) if data.node_id == peer.node_id => nodes.push(data),
				Ok(_) => errors.push(PeerError {
					node_id: peer.node_id,
					error: "peer observation node identity mismatch".into(),
				}),
				Err(e) => errors.push(PeerError {
					node_id: peer.node_id,
					error: e.to_string(),
				}),
			}
		}
		Ok(MeshResponse { nodes, errors })
	}
	pub(crate) async fn identity(&self) -> Result<Value> {
		let f = self.runtime.clone();
		Ok(json!(f.config.identity(vec![])))
	}
	pub(crate) async fn peer_discover(
		&self,
		page: PageQuery,
		mut query: Search,
	) -> Result<crate::registry::AgentPage> {
		let f = self.runtime.clone();
		query.kind = Some("agent".into());
		f.registry.legacy_agents(&query, page.offset).await
	}
	pub(crate) async fn peer_agent(&self, (id, version): (String, String)) -> Result<Entry> {
		let f = self.runtime.clone();
		f.store.require_legacy_agent(&id, &version).await?;
		let entry = f.registry.get(&id, &version).await?;
		if entry.kind != "agent" {
			return Err(Error::NotFound("agent".into()));
		}
		Ok(entry)
	}
	pub(crate) async fn peer_agent_bindings(
		&self,
		(id, version): (String, String),
	) -> Result<aidash_domain::registry::bindings::ForeignAgentSnapshot> {
		let f = &self.runtime;
		f.store.require_legacy_agent(&id, &version).await?;
		let lease = f.store.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| {
				let entry =
					crate::apps::registry::services::admission::definition(tx, &id, &version)
						.await?;
				let snapshot = crate::apps::registry::repositories::bindings::preview_remote(
					tx,
					&f.config.node_id,
					&entry,
					f.store.provider_credentials.is_some(),
				)
				.await?;
				let snapshot =
					aidash_domain::registry::bindings::ForeignAgentSnapshot::from_snapshot(
						snapshot,
					)?;
				for definition in &snapshot.definitions {
					if definition.definition.kind == "agent" {
						f.store
							.require_legacy_agent(
								&definition.identity.id,
								&definition.identity.version,
							)
							.await?;
						crate::apps::registry::repositories::bindings::preview_remote(
							tx,
							&f.config.node_id,
							&definition.definition,
							f.store.provider_credentials.is_some(),
						)
						.await?;
					}
				}
				Ok(snapshot)
			})
			.await
	}
	pub(crate) async fn peer_observe(&self, headers: HeaderMap) -> Result<Value> {
		let f = self.runtime.clone();
		let node = peer_node(&headers)?;
		Ok(serde_json::to_value(f.store.peer_observation(node).await?)?)
	}
}

use reinhardt::query::SimpleExpr;
