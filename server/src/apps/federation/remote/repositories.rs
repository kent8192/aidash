//! Native adapters that resolve and use the Nodes a sender addresses.
use aidash_application::{Result, ports::federation::FederationRepository};
use aidash_domain::federation::route::{Route, Sender};
use async_trait::async_trait;

/// The sender's way to other Nodes: the registrations of its Node and the
/// other Nodes of its Tenant.
pub(crate) struct Repository {
	pub federation: crate::federation::Federation,
	pub sender: Sender,
}

#[async_trait]
impl FederationRepository for Repository {
	async fn route(&self, node: &str) -> Result<Route> {
		self.federation
			.route(&self.sender, node)
			.await
			.map_err(Into::into)
	}
	async fn routes(&self) -> Result<Vec<Route>> {
		self.federation
			.routes(&self.sender)
			.await
			.map_err(Into::into)
	}
}

#[async_trait]
impl aidash_application::ports::federation::authority::Peers for Repository {
	async fn route(
		&self,
		node: &str,
		request: &aidash_application::federation::inbound::RequestDigest<'_>,
	) -> Result<Route> {
		self.federation
			.route_delivery(&self.sender, node, request)
			.await
			.map_err(Into::into)
	}
}

pub(crate) mod routes;

pub(crate) mod run_messages;

pub(crate) mod headroom;
