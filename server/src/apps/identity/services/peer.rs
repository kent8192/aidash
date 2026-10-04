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
use crate::{Error, Result, federation::Federation, registry::Entry};

use serde_json::json;

pub async fn write(f: &Federation, tenant: &str, input: PeerMappingInput) -> Result<PeerMapping> {
	aidash_application::authorization::peer::mappings::write(
		&crate::bootstrap::peer_mapping_repository(f),
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
	pub(crate) async fn history(
		&self,
		tenant: String,
		page: HistoryPage,
	) -> Result<Vec<MappingRevision>> {
		identifier(&tenant)?;
		crate::http::validate(&page)?;
		self.records
			.history(&tenant, page.after, page.limit as usize)
			.await
	}
	pub(crate) async fn list(&self, tenant: String, page: MappingPage) -> Result<Vec<PeerMapping>> {
		identifier(&tenant)?;
		crate::http::validate(&page)?;
		self.records
			.list(&tenant, page.offset as usize, page.limit as usize)
			.await
	}
	pub(crate) async fn set(&self, tenant: String, input: PeerMappingInput) -> Result<PeerMapping> {
		let f = self.runtime.clone();
		write(&f, &tenant, input).await
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
			let resource = access.resource("node", &f.config.node_id, json!({}));
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
