//! Management use cases.
use crate::{
	Error, Result,
	apps::federation::peer::serializers::mesh::MeshNode,
	apps::federation::peer::serializers::mesh::MeshResponse,
	apps::federation::peer::serializers::mesh::PeerError,
	authorization::{execution, identity::Actor},
	federation::{Discovery, Federation, Peer},
	registry::Search,
};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use futures_util::{StreamExt, stream};
use http::HeaderMap;
use reinhardt::injectable;
use serde_json::{Value, json};

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
		let Actor::Subject(identity) = actor else {
			// Definitions belong to Tenants; a Deployment Administrator discovers none (ADR 0029).
			return Err(Error::Forbidden);
		};
		execution::discover(&f, &identity, &query).await
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
	pub(crate) async fn peer_observe(&self, headers: HeaderMap) -> Result<Value> {
		let f = self.runtime.clone();
		let node = peer_node(&headers)?;
		// A peer observes the Runs it is home to, in any Tenant, until #241.
		let scope = TenancyScope::Deployment(DeploymentScopeReason::FederationInbound);
		Ok(serde_json::to_value(
			f.store.peer_observation(&scope, node).await?,
		)?)
	}
}
