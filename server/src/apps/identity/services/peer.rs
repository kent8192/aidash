//! Explicit inbound identity mappings. Peer authentication alone grants no
//! tenant authority, and local subject bearer tokens never cross this boundary.
use crate::apps::federation::peer::services::mapping_records::PeerMappingRecords;
use reinhardt::{Depends, injectable};
#[path = "peer/admission.rs"]
pub(crate) mod admission;
#[path = "peer/discovery.rs"]
pub(crate) mod discovery;
#[path = "peer/execution.rs"]
pub(crate) mod execution;
#[path = "peer/graph.rs"]
pub(crate) mod graph;
#[path = "peer/reads.rs"]
pub(crate) mod reads;

use super::{access::Access, catalog, policy::identifier};
use crate::{Result, federation::Federation, registry::Entry};

use serde_json::json;

pub async fn write(f: &Federation, tenant: &str, input: PeerMappingInput) -> Result<PeerMapping> {
	write_as(f, tenant, input, None).await
}

/// Writes a Peer Mapping as a Deployment Administrator, or as a Tenant
/// Administrator acting through its Mapping (#359).
pub(crate) async fn write_as(
	f: &Federation,
	tenant: &str,
	input: PeerMappingInput,
	administrator: Option<std::sync::Arc<crate::apps::identity::models::TenantAdministrator>>,
) -> Result<PeerMapping> {
	aidash_application::authorization::peer::mappings::write(
		&crate::apps::identity::repositories::peer_mappings::Repository {
			runtime: f,
			administrator,
		},
		tenant,
		input,
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}

pub(crate) async fn access(
	f: &Federation,
	node: &str,
	tenant: &str,
	subject: &str,
) -> Result<Access> {
	access_mode(f, node, tenant, subject, false).await
}

// Authority RPCs distinguish a permanent denial from a transport outage while
// never reflecting a peer's response body or internal error details.
pub(crate) async fn authority_request<T: serde::de::DeserializeOwned>(
	f: &Federation,
	node: &str,
	path: &str,
	body: &serde_json::Value,
) -> Result<T> {
	crate::bootstrap::authority_peer_client(f)
		.request(node, path, body)
		.await
		.map_err(Into::into)
}

pub(crate) use crate::apps::identity::serializers::peer::{
	DiscoveryInput, HistoryPage, MappingPage, MappingRevision,
};
pub use crate::apps::identity::serializers::peer::{PeerMapping, PeerMappingInput};

use crate::apps::identity::oidc::BrowserOrigin;
use crate::apps::identity::services::tenant_administration::TenantAdministrations;
use crate::authorization::identity::Actor;
use http::HeaderMap;

#[derive(Clone)]
pub struct PeerMappings {
	pub(crate) runtime: Federation,
	records: PeerMappingRecords,
}

#[injectable(scope = "request")]
pub(crate) async fn provide_peer_mappings(
	#[inject] runtime: Federation,
	#[inject] records: Depends<PeerMappingRecords>,
) -> PeerMappings {
	PeerMappings {
		runtime,
		records: records.into_inner(),
	}
}

impl PeerMappings {
	fn administrations(&self) -> TenantAdministrations {
		TenantAdministrations {
			runtime: self.runtime.clone(),
		}
	}
	pub(crate) async fn history(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		page: HistoryPage,
	) -> Result<Vec<MappingRevision>> {
		identifier(&tenant)?;
		crate::http::validate(&page)?;
		if matches!(actor, Actor::DeploymentAdministrator) {
			return self
				.records
				.history(&tenant, page.after, page.limit as usize)
				.await;
		}
		let pool = self.runtime.store.pool.clone();
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				administrator
					.peer_mapping_history(&pool, page.after, page.limit as usize)
					.await
			})
			.await
	}
	pub(crate) async fn list(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		page: MappingPage,
	) -> Result<Vec<PeerMapping>> {
		identifier(&tenant)?;
		crate::http::validate(&page)?;
		if matches!(actor, Actor::DeploymentAdministrator) {
			return self
				.records
				.list(&tenant, page.offset as usize, page.limit as usize)
				.await;
		}
		let pool = self.runtime.store.pool.clone();
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				administrator
					.peer_mappings(&pool, page.offset as usize, page.limit as usize)
					.await
			})
			.await
	}
	pub(crate) async fn set(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		input: PeerMappingInput,
	) -> Result<PeerMapping> {
		let f = self.runtime.clone();
		if matches!(actor, Actor::DeploymentAdministrator) {
			return write(&f, &tenant, input).await;
		}
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				write_as(&f, &tenant, input, Some(administrator.clone())).await
			})
			.await
	}
	pub(crate) async fn discover(
		&self,
		headers: HeaderMap,
		input: DiscoveryInput,
	) -> Result<Vec<Entry>> {
		let f = self.runtime.clone();
		let node = crate::apps::identity::services::http_auth::peer_node(&headers)?;
		let mut access = access(&f, node, &input.tenant, &input.subject).await?;
		let result = async {
			let resource = access.resource("node", f.config.deployment.as_str(), json!({}));
			access.require(&resource, "federation.discover").await?;
			let mut search = input.search;
			search.kind = Some("agent".into());
			let mut visible = vec![];
			for entry in catalog::list_in(&mut access, &search).await? {
				if access
					.decide(&catalog::resource(&access, &entry), "agent.execute")
					.await?
				{
					visible.push(entry);
				}
			}
			Ok(visible)
		}
		.await;
		access.finish(result).await
	}
}

pub(crate) async fn access_mode(
	f: &Federation,
	node: &str,
	tenant: &str,
	subject: &str,
	exclusive: bool,
) -> Result<Access> {
	aidash_application::authorization::peer::mappings::access(
		&crate::bootstrap::peer_mapping_repository(f),
		node,
		tenant,
		subject,
		exclusive,
	)
	.await
	.map(|scope| *scope.access)
	.map_err(Into::into)
}
#[path = "peer/dependencies.rs"]
pub(crate) mod dependencies;

#[path = "peer/semantic.rs"]
pub(crate) mod semantic;
